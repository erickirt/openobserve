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

use std::collections::HashSet;

use chrono::{Datelike, Duration, TimeZone, Timelike, Utc};
use config::{
    COMPACT_OLD_DATA_STREAM_SET,
    cluster::LOCAL_NODE,
    get_config,
    meta::{
        cluster::{CompactionJobType, Role},
        stream::{ALL_STREAM_TYPES, PartitionTimeLevel, StreamType},
    },
};
use infra::{
    file_list as infra_file_list,
    schema::{get_settings, unwrap_partition_time_level},
};
#[cfg(feature = "enterprise")]
use o2_enterprise::enterprise::common::downsampling::get_matching_downsampling_rules;
use tokio::sync::mpsc;

use crate::{common::infra::cluster::get_node_from_consistent_hash, service::db};

pub mod deleted;
pub mod flatten;
pub mod merge;
pub mod retention;
pub mod stats;
pub mod worker;

/// compactor retention run steps:
pub async fn run_retention() -> Result<(), anyhow::Error> {
    let cfg = get_config();
    // check data retention
    if cfg.compact.data_retention_days <= 0 {
        return Ok(());
    }

    let now = config::utils::time::now();
    let data_lifecycle_end = now - Duration::try_days(cfg.compact.data_retention_days).unwrap();

    let orgs = db::schema::list_organizations_from_cache().await;
    for org_id in orgs {
        for stream_type in ALL_STREAM_TYPES {
            if stream_type == StreamType::EnrichmentTables {
                continue; // skip data retention for enrichment tables
            }
            let streams = db::schema::list_streams_from_cache(&org_id, stream_type).await;
            for stream_name in streams {
                let Some(node_name) =
                    get_node_from_consistent_hash(&stream_name, &Role::Compactor, None).await
                else {
                    continue; // no compactor node
                };
                if LOCAL_NODE.name.ne(&node_name) {
                    continue; // not this node
                }

                let stream_settings =
                    infra::schema::get_settings(&org_id, &stream_name, stream_type)
                        .await
                        .unwrap_or_default();
                let stream_data_retention_end = if stream_settings.data_retention > 0 {
                    now - Duration::try_days(stream_settings.data_retention).unwrap()
                } else {
                    data_lifecycle_end
                };

                let extended_retention_days = &stream_settings.extended_retention_days;
                // creates jobs to delete data
                if let Err(e) = retention::delete_by_stream(
                    &stream_data_retention_end,
                    &org_id,
                    stream_type,
                    &stream_name,
                    extended_retention_days,
                )
                .await
                {
                    log::error!(
                        "[COMPACTOR] lifecycle: delete_by_stream [{org_id}/{stream_type}/{stream_name}] error: {e}"
                    );
                }
            }
        }
    }

    // delete files
    let jobs = db::compact::retention::list().await?;
    for job in jobs {
        let columns = job.split('/').collect::<Vec<&str>>();
        let org_id = columns[0];
        let stream_type = StreamType::from(columns[1]);
        let stream_name = columns[2];
        let retention = columns[3];

        let Some(node_name) =
            get_node_from_consistent_hash(stream_name, &Role::Compactor, None).await
        else {
            continue; // no compactor node
        };
        if LOCAL_NODE.name.ne(&node_name) {
            continue; // not this node
        }

        let ret = if retention.eq("all") {
            retention::delete_all(org_id, stream_type, stream_name).await
        } else {
            let date_range = retention.split(',').collect::<Vec<&str>>();
            retention::delete_by_date(
                org_id,
                stream_type,
                stream_name,
                (date_range[0], date_range[1]),
            )
            .await
        };

        if let Err(e) = ret {
            log::error!(
                "[COMPACTOR] delete: delete [{org_id}/{stream_type}/{stream_name}] error: {e}"
            );
        }
    }

    Ok(())
}

/// Generate job for compactor
pub async fn run_generate_job(job_type: CompactionJobType) -> Result<(), anyhow::Error> {
    let orgs = db::schema::list_organizations_from_cache().await;
    for org_id in orgs {
        // check backlist
        if !db::file_list::BLOCKED_ORGS.is_empty() && db::file_list::BLOCKED_ORGS.contains(&org_id)
        {
            continue;
        }
        for stream_type in ALL_STREAM_TYPES {
            let streams = db::schema::list_streams_from_cache(&org_id, stream_type).await;
            for stream_name in streams {
                let Some(node_name) =
                    get_node_from_consistent_hash(&stream_name, &Role::Compactor, None).await
                else {
                    continue; // no compactor node
                };
                if LOCAL_NODE.name.ne(&node_name) {
                    // This needs to be done in the case when there is a new node in the cluster
                    // This will change the node that holds the stream
                    // In case this node holds the stream, we release it for the designated node
                    if let Some((offset, _)) = db::compact::files::get_offset_from_cache(
                        &org_id,
                        stream_type,
                        &stream_name,
                    )
                    .await
                    {
                        // release the stream
                        db::compact::files::set_offset(
                            &org_id,
                            stream_type,
                            &stream_name,
                            offset,
                            None,
                        )
                        .await?;
                    }
                    continue; // not this node
                }

                // check if we are allowed to merge or just skip
                if db::compact::retention::is_deleting_stream(
                    &org_id,
                    stream_type,
                    &stream_name,
                    None,
                ) {
                    log::warn!(
                        "[COMPACTOR] the stream [{}/{}/{}] is deleting, just skip",
                        &org_id,
                        stream_type,
                        &stream_name,
                    );
                    continue;
                }

                match job_type {
                    CompactionJobType::Current => {
                        if let Err(e) =
                            merge::generate_job_by_stream(&org_id, stream_type, &stream_name).await
                        {
                            log::error!(
                                "[COMPACTOR] generate_job_by_stream [{org_id}/{stream_type}/{stream_name}] error: {e}"
                            );
                        }
                    }
                    CompactionJobType::Historical => {
                        if !COMPACT_OLD_DATA_STREAM_SET.is_empty()
                            && !COMPACT_OLD_DATA_STREAM_SET.contains(&stream_name)
                        {
                            continue;
                        }
                        if let Err(e) = merge::generate_old_data_job_by_stream(
                            &org_id,
                            stream_type,
                            &stream_name,
                        )
                        .await
                        {
                            log::error!(
                                "[COMPACTOR] generate_old_data_job_by_stream [{org_id}/{stream_type}/{stream_name}] error: {e}"
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Generate downsampling job for Metrics
#[cfg(feature = "enterprise")]
pub async fn run_generate_downsampling_job() -> Result<(), anyhow::Error> {
    let orgs = db::schema::list_organizations_from_cache().await;
    for org_id in orgs {
        // check backlist
        if !db::file_list::BLOCKED_ORGS.is_empty() && db::file_list::BLOCKED_ORGS.contains(&org_id)
        {
            continue;
        }
        let stream_type = StreamType::Metrics;
        let streams = db::schema::list_streams_from_cache(&org_id, stream_type).await;
        for stream_name in streams {
            let Some(node_name) =
                get_node_from_consistent_hash(&stream_name, &Role::Compactor, None).await
            else {
                continue; // no compactor node
            };
            let downsampling_rules = get_matching_downsampling_rules(&stream_name);
            for rule in downsampling_rules {
                if LOCAL_NODE.name.ne(&node_name) {
                    // Check if this node holds the stream
                    if let Some((offset, _)) = db::compact::downsampling::get_offset_from_cache(
                        &org_id,
                        stream_type,
                        &stream_name,
                        (rule.offset, rule.step),
                    )
                    .await
                    {
                        // release the stream
                        db::compact::downsampling::set_offset(
                            &org_id,
                            stream_type,
                            &stream_name,
                            (rule.offset, rule.step),
                            offset,
                            None,
                        )
                        .await?;
                    }
                    continue; // not this node
                }

                // check if we are allowed to merge or just skip
                if db::compact::retention::is_deleting_stream(
                    &org_id,
                    stream_type,
                    &stream_name,
                    None,
                ) {
                    log::warn!(
                        "[DOWNSAMPLING] the stream [{}/{}/{}] is deleting, just skip",
                        &org_id,
                        stream_type,
                        &stream_name,
                    );
                    continue;
                }

                if let Err(e) = merge::generate_downsampling_job_by_stream_and_rule(
                    &org_id,
                    stream_type,
                    &stream_name,
                    (rule.offset, rule.step),
                )
                .await
                {
                    log::error!(
                        "[DOWNSAMPLING] generate_downsampling_job_by_stream_and_rule [{org_id}/{stream_type}/{stream_name}] rule: {rule:?} error: {e}"
                    );
                }
            }
        }
    }

    Ok(())
}

/// compactor merging
pub async fn run_merge(job_tx: mpsc::Sender<worker::MergeJob>) -> Result<(), anyhow::Error> {
    let cfg = get_config();
    let mut jobs =
        infra_file_list::get_pending_jobs(&LOCAL_NODE.uuid, cfg.compact.batch_size).await?;
    if jobs.is_empty() {
        return Ok(());
    }

    let now = config::utils::time::now();
    let data_lifecycle_end = now - Duration::try_days(cfg.compact.data_retention_days).unwrap();

    // check the stream, if the stream partition_time_level is daily or compact step secs less than
    // 1 hour, we only allow one compactor to working on it
    let mut need_release_ids = Vec::new();
    let mut need_done_ids = Vec::new();
    for job in jobs.iter() {
        let columns = job.stream.split('/').collect::<Vec<&str>>();
        assert_eq!(columns.len(), 3);
        let org_id = columns[0].to_string();
        let stream_type = StreamType::from(columns[1]);
        let stream_name = columns[2].to_string();
        let stream_settings = get_settings(&org_id, &stream_name, stream_type)
            .await
            .unwrap_or_default();
        let partition_time_level =
            unwrap_partition_time_level(stream_settings.partition_time_level, stream_type);
        // to avoid compacting conflict with retention, need check the data retention time
        let stream_data_retention_end = if stream_settings.data_retention > 0 {
            now - Duration::try_days(stream_settings.data_retention).unwrap()
        } else {
            data_lifecycle_end
        };
        if job.offsets <= stream_data_retention_end.timestamp_micros() {
            need_done_ids.push(job.id); // the data will be deleted by retention, just skip
            continue;
        }
        if partition_time_level == PartitionTimeLevel::Daily {
            // check if this stream need process by this node
            let Some(node_name) =
                get_node_from_consistent_hash(&stream_name, &Role::Compactor, None).await
            else {
                continue; // no compactor node
            };
            if LOCAL_NODE.name.ne(&node_name) {
                need_release_ids.push(job.id); // not this node
            }
        }
    }

    if !need_done_ids.is_empty() {
        // set those jobs to done
        if let Err(e) = infra_file_list::set_job_done(&need_done_ids).await {
            log::error!("[COMPACTOR] set_job_done failed: {e}");
        }
        let need_done_ids = need_done_ids.into_iter().collect::<HashSet<_>>();
        jobs.retain(|job| !need_done_ids.contains(&job.id));
    }

    if !need_release_ids.is_empty() {
        // release those jobs
        if let Err(e) = infra_file_list::set_job_pending(&need_release_ids).await {
            log::error!("[COMPACTOR] set_job_pending failed: {e}");
        }
        let need_release_ids = need_release_ids.into_iter().collect::<HashSet<_>>();
        jobs.retain(|job| !need_release_ids.contains(&job.id));
    }

    // create a thread to keep updating the job status
    //
    // Update job status (updated_at) to prevent pickup by another node
    // convert job_timeout from secs to micros, and check 1/4 of job_timeout
    // why 1/4 of job_run_timeout?
    // because the timeout is for the entire job, we need to update the job status
    // before it timeout, using 1/2 might still risk a timeout, so we use 1/4 for safety
    let ttl = std::cmp::max(60, cfg.compact.job_run_timeout / 4) as u64;
    let job_ids = jobs.iter().map(|job| job.id).collect::<Vec<_>>();
    let (_tx, mut rx) = mpsc::channel::<()>(1);
    tokio::task::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(ttl)) => {}
                _ = rx.recv() => {
                    log::debug!("[COMPACTOR] update_running_jobs done");
                    return;
                }
            }
            if let Err(e) = infra_file_list::update_running_jobs(&job_ids).await {
                log::error!("[COMPACTOR] update_job_status failed: {e}");
            }
        }
    });

    for job in jobs {
        if job.offsets == 0 {
            log::error!("[COMPACTOR] merge job offset error: {}", job.offsets);
            continue;
        }

        let columns = job.stream.split('/').collect::<Vec<&str>>();
        assert_eq!(columns.len(), 3);
        let org_id = columns[0].to_string();
        let stream_type = StreamType::from(columns[1]);
        let stream_name = columns[2].to_string();

        // check if we are allowed to merge or just skip
        if db::compact::retention::is_deleting_stream(&org_id, stream_type, &stream_name, None) {
            log::warn!(
                "[COMPACTOR] the stream [{}/{}/{}] is deleting, just skip",
                &org_id,
                stream_type,
                &stream_name,
            );
            continue;
        }

        if let Err(e) = job_tx
            .send(worker::MergeJob {
                org_id: org_id.to_string(),
                stream_type,
                stream_name: stream_name.to_string(),
                job_id: job.id,
                offset: job.offsets,
            })
            .await
        {
            log::error!(
                "[COMPACTOR] send merge job to worker failed [{org_id}/{stream_type}/{stream_name}] error: {e}"
            );
        }
    }

    Ok(())
}

/// compactor delay delete files run steps:
/// 1. get pending deleted files from file_list_deleted table, created_at > 2 hours
/// 2. delete files from storage
pub async fn run_delay_deletion() -> Result<(), anyhow::Error> {
    let now = Utc::now();
    let time_max =
        now - Duration::try_hours(get_config().compact.delete_files_delay_hours).unwrap();
    let time_max = Utc
        .with_ymd_and_hms(
            time_max.year(),
            time_max.month(),
            time_max.day(),
            time_max.hour(),
            0,
            0,
        )
        .unwrap();
    let time_max = time_max.timestamp_micros();
    let orgs = db::schema::list_organizations_from_cache().await;
    for org_id in orgs {
        loop {
            match deleted::delete(&org_id, time_max).await {
                Ok(affected) => {
                    if affected == 0 {
                        break;
                    }
                    log::debug!("[COMPACTOR] deleted from file_list_deleted {affected} files");
                }
                Err(e) => {
                    log::error!("[COMPACTOR] delete files error: {e}");
                    break;
                }
            };
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        // update offset
        db::compact::organization::set_offset(
            &org_id,
            "file_list_deleted",
            time_max,
            Some(&LOCAL_NODE.uuid.clone()),
        )
        .await?;
    }

    Ok(())
}
