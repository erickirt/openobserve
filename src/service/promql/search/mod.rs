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

use std::{
    cmp::{max, min},
    sync::Arc,
};

use config::{
    get_config,
    meta::{
        cluster::RoleGroup,
        search::ScanStats,
        self_reporting::usage::{RequestStats, UsageType},
        stream::StreamType,
    },
    utils::{
        rand::generate_random_string,
        time::{now_micros, second_micros},
    },
};
use futures::future::try_join_all;
use hashbrown::HashMap;
use infra::errors::{Error, ErrorCodes, Result};
use proto::cluster_rpc;
use tracing::{Instrument, info_span};

use crate::{
    common::infra::cluster,
    service::{
        grpc::make_grpc_metrics_client,
        promql::{
            DEFAULT_LOOKBACK, DEFAULT_MAX_POINTS_PER_SERIES, MetricsQueryRequest, adjust_start_end,
            micros, value::*,
        },
        search::server_internal_error,
        self_reporting::report_request_usage_stats,
    },
};

mod cache;
pub mod grpc;

pub async fn init() -> Result<()> {
    if !config::cluster::LOCAL_NODE.is_querier() {
        return Ok(());
    }
    if let Err(e) = cache::init().await {
        log::error!("Error init metrics disk cache: {e}");
    }
    Ok(())
}

#[tracing::instrument(skip_all, fields(org_id = org_id))]
pub async fn search(
    trace_id: &str,
    org_id: &str,
    req: &MetricsQueryRequest,
    user_email: &str,
    timeout: i64,
) -> Result<Value> {
    let mut req: cluster_rpc::MetricsQueryRequest = req.to_owned().into();
    req.org_id = org_id.to_string();
    req.timeout = timeout;
    search_in_cluster(trace_id, req, user_email).await
}

#[tracing::instrument(name = "promql:search:cluster", skip_all, fields(org_id = req.org_id))]
async fn search_in_cluster(
    trace_id: &str,
    req: cluster_rpc::MetricsQueryRequest,
    user_email: &str,
) -> Result<Value> {
    let op_start = std::time::Instant::now();
    let started_at = now_micros();
    let cfg = get_config();

    let &cluster_rpc::MetricsQueryStmt {
        ref query,
        start,
        end,
        step,
        query_exemplars,
    } = req.query.as_ref().unwrap();

    // cache disabled if result cache is disabled or no_cache is true or start == end or step == 0
    let cache_disabled =
        !cfg.common.metrics_cache_enabled || req.no_cache || start == end || step == 0;
    // adjust start and end time
    let (start, end) = adjust_start_end(start, end, step, cache_disabled);

    log::info!(
        "[trace_id {trace_id}] promql->search->start: org_id: {}, no_cache: {}, time_range: [{},{}), step: {}, query: {}",
        req.org_id,
        cache_disabled,
        start,
        end,
        step,
        query,
    );

    // get querier nodes from cluster
    let mut nodes = cluster::get_cached_online_querier_nodes(Some(RoleGroup::Interactive))
        .await
        .unwrap();
    // sort nodes by node_id this will improve hit cache ratio
    nodes.sort_by(|a, b| a.grpc_addr.cmp(&b.grpc_addr));
    nodes.dedup_by(|a, b| a.grpc_addr == b.grpc_addr);
    nodes.sort_by_key(|x| x.id);
    let nodes = nodes;
    if nodes.is_empty() {
        return Err(Error::ErrorCode(ErrorCodes::ServerInternalError(
            "no querier node found".to_string(),
        )));
    }
    let nr_queriers = nodes.len() as i64;

    // The number of resolution steps; see the diagram at
    // https://promlabs.com/blog/2020/06/18/the-anatomy-of-a-promql-query/#range-queries
    let partition_step = max(micros(DEFAULT_LOOKBACK), step);
    let nr_steps = match (end - start) / partition_step {
        0 => 1,
        n => n,
    };

    // get cache data
    let original_start = start;
    let (start, cached_values) = if cache_disabled {
        (start, vec![])
    } else {
        let start_time = std::time::Instant::now();
        match cache::get(query, start, end, step).await {
            Ok(Some((new_start, values))) => {
                let took = start_time.elapsed().as_millis() as i32;
                config::metrics::QUERY_METRICS_CACHE_RATIO
                    .with_label_values(&[&req.org_id])
                    .observe((new_start - start) as f64 / (end - start) as f64);
                log::info!(
                    "[trace_id {trace_id}] promql->search->cache: hit cache, took: {took} ms"
                );
                (new_start, values)
            }
            Ok(None) => (start, vec![]),
            Err(err) => {
                log::error!("[trace_id {trace_id}] promql->search->cache: get cache err: {err:?}");
                (start, vec![])
            }
        }
    };

    // cache hits and full cache found
    if start > end && !cached_values.is_empty() {
        log::info!("[trace_id {trace_id}] promql->search->cache: hit full cache");
        let values = if query_exemplars {
            merge_exemplars_query(&cached_values)
        } else {
            merge_matrix_query(&cached_values)
        };
        return Ok(values);
    }

    let max_points = if cfg.limit.metrics_max_points_per_series > 0 {
        cfg.limit.metrics_max_points_per_series
    } else {
        DEFAULT_MAX_POINTS_PER_SERIES
    };
    if (end - start) / step > max_points as i64 {
        return Err(Error::ErrorCode(ErrorCodes::InvalidParams(
            "too many points per series must be returned on the given, you can change the limit by ZO_METRICS_MAX_POINTS_PER_SERIES".to_string(),
        )));
    }

    // A span of time covered by an individual querier (worker).
    let worker_dt = if nr_steps > nr_queriers {
        partition_step * ((nr_steps + nr_queriers - 1) / nr_queriers)
    } else {
        partition_step
    };

    let job = cluster_rpc::Job {
        trace_id: trace_id.to_string(),
        job: generate_random_string(7),
        stage: 0,
        partition: 0,
    };

    // make cluster request
    let mut tasks = Vec::new();
    let mut worker_start = start;
    for node in nodes.iter() {
        let node = node.clone();
        if worker_start > end {
            break;
        }
        let job = Some(cluster_rpc::Job {
            partition: node.id as _,
            ..job.clone()
        });
        let mut req = cluster_rpc::MetricsQueryRequest { job, ..req.clone() };
        let req_query = req.query.as_mut().unwrap();
        req_query.start = worker_start;
        req_query.end = min(end, worker_start + worker_dt);
        // if the end time is within the last 3 retention time, we need to fetch wal data
        if req_query.end
            >= now_micros() - second_micros(cfg.limit.max_file_retention_time as i64 * 3)
        {
            req.need_wal = true;
        }
        let req_need_wal = req.need_wal;
        worker_start += worker_dt;

        log::info!(
            "[trace_id {trace_id}] promql->search->partition: node: {}, need_wal: {}, time_range: [{},{})",
            &node.grpc_addr,
            req_need_wal,
            req_query.start,
            req_query.end,
        );

        let trace_id = trace_id.to_string();
        let grpc_span = info_span!("promql:search:cluster:grpc_search", org_id = req.org_id);
        let task = tokio::task::spawn(
            async move {
                let node = Arc::new(node) as _;
                let org_id = req.org_id.clone();
                let mut request = tonic::Request::new(req);
                let mut client = make_grpc_metrics_client(&trace_id, &org_id, &mut request, &node)
                    .await?;
                let response: cluster_rpc::MetricsQueryResponse = match client.query(request).await
                {
                    Ok(res) => res.into_inner(),
                    Err(err) => {
                        log::error!(
                            "[trace_id {trace_id}] promql->search->grpc: node: {}, search err: {:?}",
                            &node.get_grpc_addr(),
                            err
                        );
                        let err = ErrorCodes::from_json(err.message())
                            .unwrap_or(ErrorCodes::ServerInternalError(err.to_string()));
                        return Err(Error::ErrorCode(err));
                    }
                };
                let scan_stats = response.scan_stats.as_ref().unwrap();

                log::info!(
                    "[trace_id {trace_id}] promql->search->grpc: result node: {}, need_wal: {}, files: {}, scan_size: {} bytes, took: {} ms",
                    &node.get_grpc_addr(),
                    req_need_wal,
                    scan_stats.files,
                    scan_stats.original_size,
                    response.took,
                );
                Ok(response)
            }
            .instrument(grpc_span),
        );
        tasks.push(task);
    }

    let mut results = Vec::new();
    let task_results = match try_join_all(tasks).await {
        Ok(res) => res,
        Err(err) => {
            return Err(Error::ErrorCode(ErrorCodes::ServerInternalError(
                err.to_string(),
            )));
        }
    };
    for res in task_results {
        results.push(res?);
    }

    // merge multiple instances data
    let mut scan_stats = ScanStats::new();
    let mut result_type = String::new();
    let mut series_data: Vec<cluster_rpc::Series> = Vec::new();
    for resp in results {
        scan_stats.add(&resp.scan_stats.as_ref().unwrap().into());
        if result_type.is_empty() {
            result_type = resp.result_type.clone();
        }
        resp.series.into_iter().for_each(|series| {
            series_data.push(series);
        });
    }

    // add cached values to series_data
    cached_values.into_iter().for_each(|series| {
        series_data.push(series);
    });

    // with cache maybe we only get the last point from original data, then the result_type will
    // return as vector, but if the query is range query, the result_type should be matrix
    if result_type == "vector" && original_start != end {
        result_type = "matrix".to_string();
    }

    // merge result
    let values = if result_type == "matrix" {
        merge_matrix_query(&series_data)
    } else if result_type == "vector" {
        merge_vector_query(&series_data)
    } else if result_type == "scalar" {
        merge_scalar_query(&series_data)
    } else if result_type == "exemplars" {
        merge_exemplars_query(&series_data)
    } else {
        return Err(server_internal_error("invalid result type"));
    };
    log::info!(
        "[trace_id {trace_id}] promql->search->result: files: {}, scan_size: {} bytes, took: {} ms",
        scan_stats.files,
        scan_stats.original_size,
        op_start.elapsed().as_millis(),
    );

    let req_stats = RequestStats {
        records: scan_stats.records,
        size: scan_stats.original_size as f64,
        response_time: op_start.elapsed().as_secs_f64(),
        request_body: Some(query.to_string()),
        user_email: Some(user_email.to_string()),
        min_ts: Some(start),
        max_ts: Some(end),
        trace_id: Some(trace_id.to_string()),
        ..Default::default()
    };

    report_request_usage_stats(
        req_stats,
        &req.org_id,
        "", // TODO see if we can add metric name
        StreamType::Metrics,
        UsageType::MetricSearch,
        0,
        started_at,
    )
    .await;

    // cache the result
    if !cache_disabled
        && let Some(matrix) = values.get_ref_matrix_values()
        && let Err(err) = cache::set(
            trace_id,
            &req.org_id,
            query,
            original_start,
            end,
            step,
            matrix.to_vec(),
        )
        .await
    {
        log::error!("[trace_id {trace_id}] promql->search->cache: set cache err: {err:?}");
    }

    Ok(values)
}

fn merge_matrix_query(series: &[cluster_rpc::Series]) -> Value {
    let mut merged_data = HashMap::new();
    let mut merged_metrics = HashMap::new();
    for ser in series {
        let labels: Labels = ser
            .metric
            .iter()
            .map(|v| Arc::new(Label::from(v)))
            .collect();
        let entry = merged_data
            .entry(signature(&labels))
            .or_insert_with(HashMap::new);
        ser.samples.iter().for_each(|v| {
            entry.insert(v.time, v.value);
        });
        merged_metrics.insert(signature(&labels), labels);
    }
    let mut merged_data = merged_data
        .into_iter()
        .map(|(sig, samples)| {
            let mut samples = samples
                .into_iter()
                .map(|(ts, v)| Sample {
                    timestamp: ts,
                    value: v,
                })
                .collect::<Vec<_>>();
            samples.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
            (
                sig,
                RangeValue::new(merged_metrics.get(&sig).unwrap().to_owned(), samples),
            )
        })
        .collect::<Vec<_>>();
    // sort by signature
    merged_data.sort_by(|a, b| a.0.cmp(&b.0));
    let merged_data = merged_data.into_iter().map(|(_, v)| v).collect::<Vec<_>>();

    let mut value = Value::Matrix(merged_data);
    value.sort();
    value
}

fn merge_vector_query(series: &[cluster_rpc::Series]) -> Value {
    let mut merged_data = HashMap::new();
    let mut merged_metrics: HashMap<u64, Vec<Arc<Label>>> = HashMap::new();
    for ser in series {
        let labels: Labels = ser
            .metric
            .iter()
            .map(|l| Arc::new(Label::from(l)))
            .collect();
        if let Some(sample) = ser.sample.as_ref() {
            let sample: Sample = sample.into();
            merged_data.insert(signature(&labels), sample);
            merged_metrics.insert(signature(&labels), labels);
        }
    }
    let merged_data = merged_data
        .into_iter()
        .map(|(sig, sample)| InstantValue {
            labels: merged_metrics.get(&sig).unwrap().to_owned(),
            sample,
        })
        .collect::<Vec<_>>();

    let mut value = Value::Vector(merged_data);
    value.sort();
    value
}

fn merge_scalar_query(series: &[cluster_rpc::Series]) -> Value {
    let mut sample: Sample = Default::default();
    for ser in series {
        if let Some(x) = ser.sample.as_ref() {
            sample = x.into();
        } else if let Some(x) = ser.scalar {
            sample.value = x;
        }
    }
    Value::Sample(sample)
}

fn merge_exemplars_query(series: &[cluster_rpc::Series]) -> Value {
    let mut merged_data = HashMap::new();
    let mut merged_metrics = HashMap::new();
    for ser in series {
        let labels: Labels = ser
            .metric
            .iter()
            .map(|v| Arc::new(Label::from(v)))
            .collect();
        let entry = merged_data
            .entry(signature(&labels))
            .or_insert_with(HashMap::new);
        ser.exemplars
            .as_ref()
            .unwrap()
            .exemplars
            .iter()
            .for_each(|v| {
                entry.insert(v.time, v);
            });
        merged_metrics.insert(signature(&labels), labels);
    }
    let merged_data = merged_data
        .into_iter()
        .map(|(sig, exemplars)| {
            let mut exemplars: Vec<Arc<Exemplar>> = exemplars
                .into_iter()
                .map(|(_ts, v)| Arc::new(v.into()))
                .collect::<Vec<_>>();
            exemplars.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
            RangeValue::new_with_exemplars(merged_metrics.get(&sig).unwrap().to_owned(), exemplars)
        })
        .collect();

    let mut value = Value::Matrix(merged_data);
    value.sort();
    value
}
