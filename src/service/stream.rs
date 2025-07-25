// Copyright 2025 OpenObserve Inc.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::io::Error;

use actix_web::{HttpResponse, http, http::StatusCode};
use arrow_schema::DataType;
use config::{
    SIZE_IN_MB, SQL_FULL_TEXT_SEARCH_FIELDS, TIMESTAMP_COL_NAME, get_config, is_local_disk_storage,
    meta::{
        promql,
        stream::{
            DistinctField, StreamParams, StreamSettings, StreamStats, StreamType,
            UpdateStreamSettings,
        },
    },
    utils::{json, time::now_micros},
};
use datafusion::arrow::datatypes::Schema;
use hashbrown::HashMap;
use infra::{
    cache::stats,
    schema::{
        STREAM_RECORD_ID_GENERATOR, STREAM_SCHEMAS, STREAM_SCHEMAS_LATEST, STREAM_SETTINGS,
        unwrap_partition_time_level, unwrap_stream_created_at, unwrap_stream_is_derived,
        unwrap_stream_settings,
    },
    table::distinct_values::{DistinctFieldRecord, OriginType, check_field_use},
};
#[cfg(feature = "enterprise")]
use o2_enterprise::enterprise::re_patterns::PATTERN_MANAGER;

use super::db::enrichment_table;
#[cfg(feature = "enterprise")]
use crate::service::db::re_pattern::process_association_changes;
use crate::{
    common::meta::{
        authz::Authz,
        http::HttpResponse as MetaHttpResponse,
        stream::{Stream, StreamProperty},
    },
    handler::http::router::ERROR_HEADER,
    service::{
        db::{self, distinct_values},
        metrics::get_prom_metadata_from_schema,
    },
};

const LOCAL: &str = "disk";
const S3: &str = "s3";

pub async fn get_stream(
    org_id: &str,
    stream_name: &str,
    stream_type: StreamType,
) -> Option<Stream> {
    let schema = infra::schema::get(org_id, stream_name, stream_type)
        .await
        .unwrap();

    if schema != Schema::empty() {
        let mut stats = stats::get_stream_stats(org_id, stream_name, stream_type);
        transform_stats(&mut stats, org_id, stream_name, stream_type).await;
        Some(stream_res(
            org_id,
            stream_name,
            stream_type,
            schema,
            Some(stats),
        ))
    } else {
        None
    }
}

pub async fn get_streams(
    org_id: &str,
    stream_type: Option<StreamType>,
    fetch_schema: bool,
    permitted_streams: Option<Vec<String>>,
) -> Vec<Stream> {
    let indices = db::schema::list(org_id, stream_type, fetch_schema)
        .await
        .unwrap_or_default();

    let filtered_indices = if let Some(s_type) = stream_type {
        let s_type = match s_type {
            StreamType::EnrichmentTables => "enrichment_table",
            _ => s_type.as_str(),
        };
        match permitted_streams {
            Some(permitted_streams) => {
                if permitted_streams.contains(&format!("{s_type}:_all_{org_id}")) {
                    indices
                } else {
                    indices
                        .into_iter()
                        .filter(|stream_loc| {
                            permitted_streams
                                .contains(&format!("{}:{}", s_type, stream_loc.stream_name))
                        })
                        .collect::<Vec<_>>()
                }
            }
            None => indices,
        }
    } else {
        indices
    };
    let mut indices_res = Vec::with_capacity(filtered_indices.len());
    for stream_loc in filtered_indices {
        let mut stats = stats::get_stream_stats(
            org_id,
            stream_loc.stream_name.as_str(),
            stream_loc.stream_type,
        );
        if stats.eq(&StreamStats::default())
            && stream_loc.stream_type != StreamType::EnrichmentTables
        {
            indices_res.push(stream_res(
                org_id,
                stream_loc.stream_name.as_str(),
                stream_loc.stream_type,
                stream_loc.schema,
                None,
            ));
        } else {
            transform_stats(
                &mut stats,
                org_id,
                stream_loc.stream_name.as_str(),
                stream_loc.stream_type,
            )
            .await;
            indices_res.push(stream_res(
                org_id,
                stream_loc.stream_name.as_str(),
                stream_loc.stream_type,
                stream_loc.schema,
                Some(stats),
            ));
        }
    }
    indices_res
}

// org_id is only for pattern associations, which is ent only
pub fn stream_res(
    _org_id: &str,
    stream_name: &str,
    stream_type: StreamType,
    schema: Schema,
    stats: Option<StreamStats>,
) -> Stream {
    let storage_type = if is_local_disk_storage() { LOCAL } else { S3 };
    let mappings = schema
        .fields()
        .iter()
        .map(|field| StreamProperty {
            prop_type: field.data_type().to_string(),
            name: field.name().to_string(),
        })
        .collect::<Vec<_>>();

    let mut stats = stats.unwrap_or_default();
    stats.created_at = unwrap_stream_created_at(&schema).unwrap_or_default();

    let metrics_meta = if stream_type == StreamType::Metrics {
        let mut meta = get_prom_metadata_from_schema(&schema).unwrap_or(promql::Metadata {
            metric_type: promql::MetricType::Empty,
            metric_family_name: stream_name.to_string(),
            help: stream_name.to_string(),
            unit: "".to_string(),
        });
        if meta.metric_type == promql::MetricType::Empty
            && (stream_name.ends_with("_bucket")
                || stream_name.ends_with("_sum")
                || stream_name.ends_with("_count"))
        {
            meta.metric_type = promql::MetricType::Counter;
        }
        Some(meta)
    } else {
        None
    };

    let mut settings = unwrap_stream_settings(&schema).unwrap_or_default();
    if settings == StreamSettings::default() {
        settings.approx_partition = get_config()
            .common
            .use_stream_settings_for_partitions_enabled;
    }

    settings.partition_time_level = Some(unwrap_partition_time_level(
        settings.partition_time_level,
        stream_type,
    ));

    #[cfg(not(feature = "enterprise"))]
    let pattern_associations = vec![];
    // because this fn cannot be async, we cannot await on initializing the pattern
    // manager. So instead we do it in best-effort-way, where if it is already initialized,
    // we get the patterns, otherwise report them as empty
    #[cfg(feature = "enterprise")]
    let pattern_associations = match PATTERN_MANAGER.get() {
        Some(m) => m.get_associations(_org_id, stream_type, stream_name),
        None => vec![],
    };
    let is_derived = unwrap_stream_is_derived(&schema);

    Stream {
        name: stream_name.to_string(),
        storage_type: storage_type.to_string(),
        stream_type,
        total_fields: mappings.len(),
        schema: mappings,
        uds_schema: vec![],
        stats,
        settings,
        metrics_meta,
        pattern_associations,
        is_derived,
    }
}

#[tracing::instrument(skip(settings))]
pub async fn save_stream_settings(
    org_id: &str,
    stream_name: &str,
    stream_type: StreamType,
    mut settings: StreamSettings,
) -> Result<HttpResponse, Error> {
    let cfg = config::get_config();
    // check if we are allowed to ingest
    if db::compact::retention::is_deleting_stream(org_id, stream_type, stream_name, None) {
        return Ok(HttpResponse::BadRequest()
            .append_header((
                ERROR_HEADER,
                format!("stream [{stream_name}] is being deleted"),
            ))
            .json(MetaHttpResponse::error(
                http::StatusCode::BAD_REQUEST,
                format!("stream [{stream_name}] is being deleted"),
            )));
    }

    // only allow setting user defined schema for logs stream
    if stream_type != StreamType::Logs && !settings.defined_schema_fields.is_empty() {
        return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
            http::StatusCode::BAD_REQUEST,
            "only logs stream can have user defined schema",
        )));
    }

    // _all field can't setting for inverted index & index field
    for key in settings.full_text_search_keys.iter() {
        if key == &cfg.common.column_all {
            return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                http::StatusCode::BAD_REQUEST,
                format!("field [{key}] can't be used for full text search"),
            )));
        }
    }
    for key in settings.index_fields.iter() {
        if key == &cfg.common.column_all {
            return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                http::StatusCode::BAD_REQUEST,
                format!("field [{key}] can't be used for secondary index"),
            )));
        }
    }

    for key in settings.partition_keys.iter() {
        if SQL_FULL_TEXT_SEARCH_FIELDS.contains(&key.field) || key.field == cfg.common.column_all {
            return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                http::StatusCode::BAD_REQUEST,
                format!("field [{}] can't be used for partition key", key.field),
            )));
        }
    }

    // get schema
    let schema = match infra::schema::get(org_id, stream_name, stream_type).await {
        Ok(schema) => schema,
        Err(e) => {
            return Ok(HttpResponse::InternalServerError()
                .append_header((ERROR_HEADER, format!("error in getting schema : {e}")))
                .json(MetaHttpResponse::error(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("error in getting schema : {e}"),
                )));
        }
    };
    let schema_fields = schema
        .fields()
        .iter()
        .map(|f| (f.name(), f))
        .collect::<HashMap<_, _>>();

    // check the full text search keys must be text field
    for key in settings.full_text_search_keys.iter() {
        let Some(field) = schema_fields.get(key) else {
            return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                http::StatusCode::BAD_REQUEST,
                format!("field [{key}] not found in schema"),
            )));
        };
        if field.data_type() != &DataType::Utf8 {
            return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                http::StatusCode::BAD_REQUEST,
                format!("full text search field [{key}] must be text field"),
            )));
        }
    }

    // we need to keep the old partition information, because the hash bucket num can't be changed
    // get old settings and then update partition_keys
    let mut old_partition_keys = unwrap_stream_settings(&schema)
        .unwrap_or_default()
        .partition_keys;
    // first disable all old partition keys
    for v in old_partition_keys.iter_mut() {
        v.disabled = true;
    }
    // then update new partition keys
    for v in settings.partition_keys.iter() {
        if let Some(old_field) = old_partition_keys.iter_mut().find(|k| k.field == v.field) {
            if old_field.types != v.types {
                return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                    http::StatusCode::BAD_REQUEST,
                    format!("field [{}] partition types can't be changed", v.field),
                )));
            }
            old_field.disabled = v.disabled;
        } else {
            old_partition_keys.push(v.clone());
        }
    }
    settings.partition_keys = old_partition_keys;

    for range in settings.extended_retention_days.iter() {
        if range.start > range.end {
            return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                http::StatusCode::BAD_REQUEST,
                "start day should be less than end day",
            )));
        }
    }

    let mut metadata = schema.metadata.clone();
    metadata.insert("settings".to_string(), json::to_string(&settings).unwrap());
    if !metadata.contains_key("created_at") {
        metadata.insert("created_at".to_string(), now_micros().to_string());
    }
    db::schema::update_setting(org_id, stream_name, stream_type, metadata)
        .await
        .unwrap();

    Ok(HttpResponse::Ok().json(MetaHttpResponse::message(http::StatusCode::OK, "")))
}

#[tracing::instrument(skip(new_settings))]
pub async fn update_stream_settings(
    org_id: &str,
    stream_name: &str,
    stream_type: StreamType,
    new_settings: UpdateStreamSettings,
) -> Result<HttpResponse, Error> {
    let cfg = config::get_config();
    match infra::schema::get_settings(org_id, stream_name, stream_type).await {
        Some(mut settings) => {
            if let Some(max_query_range) = new_settings.max_query_range {
                settings.max_query_range = max_query_range;
            }
            if let Some(store_original_data) = new_settings.store_original_data {
                settings.store_original_data = store_original_data;
            }
            if let Some(approx_partition) = new_settings.approx_partition {
                settings.approx_partition = approx_partition;
            }

            if let Some(flatten_level) = new_settings.flatten_level {
                settings.flatten_level = Some(flatten_level);
            }

            if let Some(data_retention) = new_settings.data_retention {
                settings.data_retention = data_retention;
            }

            if let Some(index_original_data) = new_settings.index_original_data {
                settings.index_original_data = index_original_data;
            }

            if let Some(index_all_values) = new_settings.index_all_values {
                settings.index_all_values = index_all_values;
            }

            // if index_original_data is true, store_original_data must be true
            if settings.index_original_data {
                settings.store_original_data = true;
            }

            // index_original_data & index_all_values only can open one at a time
            if settings.index_original_data && settings.index_all_values {
                return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                    http::StatusCode::BAD_REQUEST,
                    "index_original_data & index_all_values cannot be true at the same time",
                )));
            }

            // check for user defined schema
            if !new_settings.defined_schema_fields.add.is_empty() {
                if !cfg.common.allow_user_defined_schemas {
                    return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                        http::StatusCode::BAD_REQUEST,
                        "user defined schema is not allowed, you need to set ZO_ALLOW_USER_DEFINED_SCHEMAS=true",
                    )));
                }
                settings
                    .defined_schema_fields
                    .extend(new_settings.defined_schema_fields.add);
            }
            if !new_settings.defined_schema_fields.remove.is_empty() {
                settings
                    .defined_schema_fields
                    .retain(|field| !new_settings.defined_schema_fields.remove.contains(field));
            }
            if settings.defined_schema_fields.len() > cfg.limit.user_defined_schema_max_fields {
                return Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
                    http::StatusCode::BAD_REQUEST,
                    format!(
                        "user defined schema fields count exceeds the limit: {}",
                        cfg.limit.user_defined_schema_max_fields
                    ),
                )));
            }

            // check for bloom filter fields
            if !new_settings.bloom_filter_fields.add.is_empty() {
                settings
                    .bloom_filter_fields
                    .extend(new_settings.bloom_filter_fields.add);
            }
            if !new_settings.bloom_filter_fields.remove.is_empty() {
                settings
                    .bloom_filter_fields
                    .retain(|field| !new_settings.bloom_filter_fields.remove.contains(field));
            }

            // check for index fields
            if !new_settings.index_fields.add.is_empty() {
                settings.index_fields.extend(new_settings.index_fields.add);
                settings.index_updated_at = now_micros();
            }
            if !new_settings.index_fields.remove.is_empty() {
                settings
                    .index_fields
                    .retain(|field| !new_settings.index_fields.remove.contains(field));
            }

            if !new_settings.extended_retention_days.add.is_empty() {
                settings
                    .extended_retention_days
                    .extend(new_settings.extended_retention_days.add);
            }

            if !new_settings.extended_retention_days.remove.is_empty() {
                settings
                    .extended_retention_days
                    .retain(|range| !new_settings.extended_retention_days.remove.contains(range));
            }

            if !new_settings.distinct_value_fields.add.is_empty() {
                for f in &new_settings.distinct_value_fields.add {
                    if f == "count" || f == TIMESTAMP_COL_NAME {
                        return Ok(HttpResponse::InternalServerError().json(
                            MetaHttpResponse::error(
                                http::StatusCode::BAD_REQUEST,
                                format!("count and {TIMESTAMP_COL_NAME} are reserved fields and cannot be added"),
                            ),
                        ));
                    }
                    // we ignore full text search fields
                    if settings.full_text_search_keys.contains(f)
                        || new_settings.full_text_search_keys.add.contains(f)
                    {
                        continue;
                    }
                    let record = DistinctFieldRecord::new(
                        OriginType::Stream,
                        stream_name,
                        org_id,
                        stream_name,
                        stream_type.to_string(),
                        f,
                    );
                    if let Err(e) = distinct_values::add(record).await {
                        return Ok(HttpResponse::InternalServerError()
                            .append_header((
                                ERROR_HEADER,
                                format!("error in updating settings : {e}"),
                            ))
                            .json(MetaHttpResponse::error(
                                http::StatusCode::INTERNAL_SERVER_ERROR,
                                format!("error in updating settings : {e}"),
                            )));
                    }
                    // we cannot allow duplicate entries here
                    let temp = DistinctField {
                        name: f.to_owned(),
                        added_ts: now_micros(),
                    };
                    if !settings.distinct_value_fields.contains(&temp) {
                        settings.distinct_value_fields.push(temp);
                    }
                }
            }

            if !new_settings.distinct_value_fields.remove.is_empty() {
                for f in &new_settings.distinct_value_fields.remove {
                    let usage =
                        match check_field_use(org_id, stream_name, stream_type.as_str(), f).await {
                            Ok(entry) => entry,
                            Err(e) => {
                                return Ok(HttpResponse::InternalServerError()
                                    .append_header((
                                        ERROR_HEADER,
                                        format!("error in updating settings : {e}"),
                                    ))
                                    .json(MetaHttpResponse::error(
                                        http::StatusCode::INTERNAL_SERVER_ERROR,
                                        format!("error in updating settings : {e}"),
                                    )));
                            }
                        };
                    // if there are multiple uses, we cannot allow it to be removed
                    if usage.len() > 1 {
                        return Ok(HttpResponse::BadRequest().json(
                            MetaHttpResponse::error(
                                http::StatusCode::BAD_REQUEST,
                                format!("error in removing distinct field : field {f} if used in dashboards/reports"),
                            ),
                        ));
                    }
                    // here we can be sure that usage is at most 1 record
                    if let Some(entry) = usage.first()
                        && entry.origin != OriginType::Stream
                    {
                        return Ok(HttpResponse::BadRequest().json(
                                MetaHttpResponse::error(
                                    http::StatusCode::BAD_REQUEST,
                                    format!("error in removing distinct field : field {f} if used in dashboards/reports"),
                                ),
                        ));
                    }
                }
                // here we are sure that all fields to be removed can be removed,
                // so we bulk filter
                settings.distinct_value_fields.retain(|field| {
                    !new_settings
                        .distinct_value_fields
                        .remove
                        .contains(&field.name)
                });
            }

            if !new_settings.full_text_search_keys.add.is_empty() {
                settings
                    .full_text_search_keys
                    .extend(new_settings.full_text_search_keys.add);
                settings.index_updated_at = now_micros();
            }

            if !new_settings.full_text_search_keys.remove.is_empty() {
                settings
                    .full_text_search_keys
                    .retain(|field| !new_settings.full_text_search_keys.remove.contains(field));
            }

            if !new_settings.partition_keys.add.is_empty() {
                settings
                    .partition_keys
                    .extend(new_settings.partition_keys.add);
            }

            if !new_settings.partition_keys.remove.is_empty() {
                settings
                    .partition_keys
                    .retain(|field| !new_settings.partition_keys.remove.contains(field));
            }

            if let Some(partition_time_level) = new_settings.partition_time_level {
                settings.partition_time_level = Some(partition_time_level);
            }

            #[cfg(feature = "enterprise")]
            {
                if let Err(e) = process_association_changes(
                    org_id,
                    stream_name,
                    stream_type,
                    new_settings.pattern_associations,
                )
                .await
                {
                    return Ok(
                        HttpResponse::InternalServerError().json(MetaHttpResponse::error(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                            format!(
                                "Internal server error while updating pattern associations {e}",
                            ),
                        )),
                    );
                }
            }

            save_stream_settings(org_id, stream_name, stream_type, settings).await
        }
        None => Ok(HttpResponse::BadRequest().json(MetaHttpResponse::error(
            http::StatusCode::BAD_REQUEST,
            "stream settings could not be found",
        ))),
    }
}

#[tracing::instrument]
pub async fn delete_stream(
    org_id: &str,
    stream_name: &str,
    stream_type: StreamType,
    del_related_feature_resources: bool,
) -> Result<HttpResponse, Error> {
    let schema = infra::schema::get_versions(org_id, stream_name, stream_type, None)
        .await
        .unwrap();
    if schema.is_empty() {
        return Ok(HttpResponse::NotFound().json(MetaHttpResponse::error(
            StatusCode::NOT_FOUND,
            "stream not found",
        )));
    }

    // delete stream schema
    if let Err(e) = db::schema::delete(org_id, stream_name, Some(stream_type)).await {
        return Ok(HttpResponse::InternalServerError()
            .append_header((ERROR_HEADER, format!("failed to delete stream schema: {e}")))
            .json(MetaHttpResponse::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to delete stream schema: {e}"),
            )));
    }

    // delete associated feature resources, i.e. pipelines, alerts
    if del_related_feature_resources {
        if let Some(pipeline) =
            db::pipeline::get_by_stream(&StreamParams::new(org_id, stream_name, stream_type)).await
            && let Err(e) = db::pipeline::delete(&pipeline.id).await
        {
            return Ok(
                HttpResponse::InternalServerError().json(MetaHttpResponse::error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "Error: failed to delete the associated pipeline \"{}\": {e}",
                        pipeline.name
                    ),
                )),
            );
        }

        if let Ok(alerts) =
            db::alerts::alert::list(org_id, Some(stream_type), Some(stream_name)).await
        {
            for alert in alerts {
                if let Err(e) =
                    db::alerts::alert::delete_by_name(org_id, stream_type, stream_name, &alert.name)
                        .await
                {
                    return Ok(
                        HttpResponse::InternalServerError().json(MetaHttpResponse::error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!(
                                "Error: failed to delete the associated alert \"{}\": {e}",
                                alert.name
                            ),
                        )),
                    );
                }
            }
        }
    }

    // delete related resource
    if let Err(e) = stream_delete_inner(org_id, stream_type, stream_name).await {
        return Ok(HttpResponse::InternalServerError()
            .append_header((ERROR_HEADER, format!("failed to delete stream: {e}")))
            .json(MetaHttpResponse::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to delete stream: {e}"),
            )));
    }

    // enrichment table cleanup

    if stream_type == StreamType::EnrichmentTables {
        crate::service::enrichment_table::cleanup_enrichment_table_resources(
            org_id,
            stream_name,
            stream_type,
        )
        .await;
    }

    // delete ownership
    crate::common::utils::auth::remove_ownership(
        org_id,
        stream_type.as_str(),
        Authz::new(stream_name),
    )
    .await;

    Ok(HttpResponse::Ok().json(MetaHttpResponse::message(StatusCode::OK, "stream deleted")))
}

pub async fn stream_delete_inner(
    org_id: &str,
    stream_type: StreamType,
    stream_name: &str,
) -> Result<(), anyhow::Error> {
    #[cfg(feature = "enterprise")]
    {
        use super::db::re_pattern::remove_stream_associations_after_deletion;
        remove_stream_associations_after_deletion(org_id, stream_name, stream_type).await?;
    }

    // create delete for compactor
    if let Err(e) =
        db::compact::retention::delete_stream(org_id, stream_type, stream_name, None).await
    {
        log::error!(
            "Failed to create retention job for stream: {org_id}/{stream_type}/{stream_name}, error: {e}"
        );
        return Err(e);
    }

    // delete stream schema cache
    let key = format!("{org_id}/{stream_type}/{stream_name}");
    let mut w = STREAM_SCHEMAS.write().await;
    w.remove(&key);
    drop(w);
    let mut w = STREAM_SCHEMAS_LATEST.write().await;
    w.remove(&key);
    drop(w);

    // delete stream settings cache
    let mut w = STREAM_SETTINGS.write().await;
    w.remove(&key);
    infra::schema::set_stream_settings_atomic(w.clone());
    drop(w);

    // delete stream record id generator cache
    {
        STREAM_RECORD_ID_GENERATOR.remove(&key);
    }

    // delete stream compaction offset
    if let Err(e) = db::compact::files::del_offset(org_id, stream_type, stream_name).await {
        log::error!(
            "Failed to delete stream compact offset for stream: {org_id}/{stream_type}/{stream_name}, error: {e}"
        );
        return Err(e);
    }

    Ok(())
}

async fn transform_stats(
    stats: &mut StreamStats,
    org_id: &str,
    stream_name: &str,
    stream_type: StreamType,
) {
    stats.storage_size /= SIZE_IN_MB;
    stats.compressed_size /= SIZE_IN_MB;
    stats.index_size /= SIZE_IN_MB;
    if stream_type == StreamType::EnrichmentTables
        && let Some(meta) = enrichment_table::get_meta_table_stats(org_id, stream_name).await
    {
        stats.doc_time_min = meta.start_time;
        stats.doc_time_max = meta.end_time;
    }
}

pub async fn delete_fields(
    org_id: &str,
    stream_name: &str,
    stream_type: Option<StreamType>,
    fields: &[String],
) -> Result<(), anyhow::Error> {
    if fields.is_empty() {
        return Ok(());
    }
    db::schema::delete_fields(
        org_id,
        stream_name,
        stream_type.unwrap_or_default(),
        fields.to_vec(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{DataType, Field};

    use super::*;

    #[test]
    fn test_stream_res() {
        let stats = StreamStats::default();
        let schema = Schema::new(vec![Field::new("f.c", DataType::Int32, false)]);
        let res = stream_res(
            "default",
            "Test",
            StreamType::Logs,
            schema,
            Some(stats.clone()),
        );
        assert_eq!(res.stats, stats);
    }
}
