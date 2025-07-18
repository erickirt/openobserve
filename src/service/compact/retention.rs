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

use std::{collections::HashMap, path::PathBuf};

use chrono::{DateTime, Duration, TimeZone, Utc};
use config::{
    cluster::LOCAL_NODE,
    get_config, is_local_disk_storage,
    meta::stream::{FileKey, FileListDeleted, PartitionTimeLevel, StreamType, TimeRange},
    utils::time::{BASE_TIME, hour_micros},
};
use infra::{cache, dist_lock, file_list as infra_file_list};
use itertools::Itertools;

use crate::{
    common::infra::cluster::get_node_by_uuid,
    service::{db, file_list},
};

/// This function will split the original time range based on the exclude range
/// It expects a mutable reference to a Vec which will be populated with the split time ranges
/// The limit for the red day retention period considered is extended_data_retention_days +
/// data_retention_days.
/// The original time range will change as per the exclude range. There are two cases which can
/// occur
/// 1. The exclude range is completely inside the original time range
///   - in this case the original time range will be updated to the last split range
/// 2. The exclude range is partially inside the original time range
///   - in this case the original time range will be updated to an empty range
///
/// Returns the number of jobs created
fn generate_time_ranges_for_deletion(
    extended_retention_ranges: Vec<TimeRange>,
    original_time_range: TimeRange,
    last_retained_time: i64,
) -> Vec<TimeRange> {
    // first we flatten out the overlapping red days
    let extended_retention_ranges_iter =
        TimeRange::flatten_overlapping_ranges(extended_retention_ranges).into_iter();

    log::debug!(
        "[COMPACTOR] populate_time_ranges_for_deletion exclude_range: {}, original_time_range: {}",
        extended_retention_ranges_iter.clone().join(", "),
        original_time_range.clone()
    );

    let filtered_extended_ranges = extended_retention_ranges_iter
        // filter out the ranges which are out of scope and mutate the ranges which are partially
        // inside the original time range
        .filter_map(|range| {
            if range.start > original_time_range.end {
                None
            } else if range.end > original_time_range.end {
                Some(TimeRange::new(
                    range.start,
                    original_time_range.end,
                ))
            } else {
                Some(range)
            }
        })
        // filter out the ranges which are older than the last retained time
        .filter_map(|range| {
            if range.end < last_retained_time {
                None
            } else if range.start < last_retained_time {
                Some(TimeRange::new(
                    last_retained_time,
                    range.end,
                ))
            } else {
                Some(range)
            }
        })
        // sort the ranges by start time
        .sorted_by(|x, x1| x.start.cmp(&x1.start))
        .collect::<Vec<TimeRange>>();

    let mut time_ranges_for_deletion = vec![];
    let mut last_split_range_opt = Some(original_time_range);

    for mut extended_range in filtered_extended_ranges {
        if let Some(ref last_split_range) = last_split_range_opt {
            if extended_range.end + hour_micros(24) != last_split_range.end {
                extended_range.end += hour_micros(24); // add one day to make it exclusive
            }
            match last_split_range.split_by_range(&extended_range) {
                Ok((Some(left), Some(right))) => {
                    time_ranges_for_deletion.push(left);
                    last_split_range_opt = Some(right);
                }
                Ok((None, Some(right))) => {
                    last_split_range_opt = Some(right);
                }
                Ok((Some(left), None)) => {
                    time_ranges_for_deletion.push(left);
                    last_split_range_opt = None;
                    break;
                }
                Ok((None, None)) => {
                    last_split_range_opt = None;
                    break;
                }
                Err(_) => {
                    // if the split fails then we can skip the range
                    continue;
                }
            }
        } else {
            break;
        }
    }

    if let Some(range) = last_split_range_opt {
        time_ranges_for_deletion.push(range);
    }

    log::debug!(
        "[COMPACTOR] populate_time_ranges_for_deletion time_ranges_for_deletion: {}",
        time_ranges_for_deletion.iter().join(", ")
    );

    time_ranges_for_deletion
}

/// Creates delete jobs for the stream based on the stream settings
/// Returns the number of jobs created
pub async fn delete_by_stream(
    lifecycle_end: &DateTime<Utc>,
    org_id: &str,
    stream_type: StreamType,
    stream_name: &str,
    extended_retentions: &[TimeRange],
) -> Result<u32, anyhow::Error> {
    // get schema
    let stats = cache::stats::get_stream_stats(org_id, stream_name, stream_type);
    let created_at = stats.doc_time_min;
    if created_at == 0 {
        return Ok(0); // no data, just skip
    }
    let created_at: DateTime<Utc> = Utc.timestamp_nanos(created_at * 1000);
    if created_at >= *lifecycle_end {
        return Ok(0); // created_at is after lifecycle end, just skip
    }

    log::debug!(
        "[COMPACTOR] delete_by_stream {}/{}/{}/{},{}",
        org_id,
        stream_type,
        stream_name,
        created_at.format("%Y-%m-%d").to_string().as_str(),
        lifecycle_end.format("%Y-%m-%d").to_string().as_str(),
    );

    if created_at.ge(lifecycle_end) {
        return Ok(0); // created_at is after lifecycle end, just skip
    }

    // last extended retention time
    let last_retained_time = (*lifecycle_end
        - Duration::try_days(get_config().compact.extended_data_retention_days).unwrap())
    .timestamp_micros();
    // time range of deletion
    let original_deletion_time_range = TimeRange::new(
        created_at.timestamp_micros(),
        lifecycle_end.timestamp_micros(),
    );

    let final_deletion_time_ranges = if extended_retentions.is_empty() {
        vec![original_deletion_time_range]
    } else {
        let ranges = generate_time_ranges_for_deletion(
            extended_retentions.to_vec(),
            original_deletion_time_range,
            last_retained_time,
        );
        log::debug!(
            "[COMPACTOR] extended_retentions: {}, final_deletion_time_ranges: {}",
            extended_retentions.iter().join(", "),
            ranges.iter().join(", ")
        );
        ranges
    };

    let job_nos = final_deletion_time_ranges.len();

    for time_range in final_deletion_time_ranges {
        let time_range_start = Utc
            .timestamp_nanos(time_range.start * 1000)
            .format("%Y-%m-%d")
            .to_string();
        let time_range_end = Utc
            .timestamp_nanos(time_range.end * 1000)
            .format("%Y-%m-%d")
            .to_string();
        if time_range_start >= time_range_end {
            continue;
        }

        log::debug!(
            "[COMPACTOR] delete_by_stream {org_id}/{stream_type}/{stream_name}/{time_range_start},{time_range_end}",
        );

        db::compact::retention::delete_stream(
            org_id,
            stream_type,
            stream_name,
            Some((time_range_start.as_str(), time_range_end.as_str())),
        )
        .await?;
    }

    Ok(job_nos as u32)
}

pub async fn delete_all(
    org_id: &str,
    stream_type: StreamType,
    stream_name: &str,
) -> Result<(), anyhow::Error> {
    let lock_key = format!("/compact/retention/{org_id}/{stream_type}/{stream_name}");
    let locker = dist_lock::lock(&lock_key, 0).await?;
    let node = db::compact::retention::get_stream(org_id, stream_type, stream_name, None).await;
    if !node.is_empty() && LOCAL_NODE.uuid.ne(&node) && get_node_by_uuid(&node).await.is_some() {
        log::warn!("[COMPACTOR] stream {org_id}/{stream_type}/{stream_name} is deleting by {node}");
        dist_lock::unlock(&locker).await?;
        return Ok(()); // not this node, just skip
    }

    // before start merging, set current node to lock the stream
    let ret = db::compact::retention::process_stream(
        org_id,
        stream_type,
        stream_name,
        None,
        &LOCAL_NODE.uuid.clone(),
    )
    .await;
    // already bind to this node, we can unlock now
    dist_lock::unlock(&locker).await?;
    drop(locker);
    ret?;

    let start_time = BASE_TIME.timestamp_micros();
    let end_time = Utc::now().timestamp_micros();

    let cfg = get_config();
    if is_local_disk_storage() {
        let data_dir = format!(
            "{}files/{org_id}/{stream_type}/{stream_name}",
            cfg.common.data_stream_dir
        );
        let path = std::path::Path::new(&data_dir);
        if path.exists() {
            tokio::fs::remove_dir_all(path).await?;
        }
        log::info!("deleted all files: {path:?}");
    }

    // delete from file list
    delete_from_file_list(org_id, stream_type, stream_name, (start_time, end_time)).await?;
    super::super::file_list_dump::delete_all_for_stream(org_id, stream_type, stream_name).await?;
    log::info!("deleted file list for: {org_id}/{stream_type}/{stream_name}/all");

    // mark delete done
    db::compact::retention::delete_stream_done(org_id, stream_type, stream_name, None).await?;
    log::info!("deleted stream all: {org_id}/{stream_type}/{stream_name}");

    Ok(())
}

pub async fn delete_by_date(
    org_id: &str,
    stream_type: StreamType,
    stream_name: &str,
    date_range: (&str, &str),
) -> Result<(), anyhow::Error> {
    let lock_key = format!("/compact/retention/{org_id}/{stream_type}/{stream_name}");
    let locker = dist_lock::lock(&lock_key, 0).await?;
    let node =
        db::compact::retention::get_stream(org_id, stream_type, stream_name, Some(date_range))
            .await;
    if !node.is_empty() && LOCAL_NODE.uuid.ne(&node) && get_node_by_uuid(&node).await.is_some() {
        log::warn!(
            "[COMPACTOR] stream {org_id}/{stream_type}/{stream_name}/{date_range:?} is deleting by {node}"
        );
        dist_lock::unlock(&locker).await?;
        return Ok(()); // not this node, just skip
    }

    // before start merging, set current node to lock the stream
    let ret = db::compact::retention::process_stream(
        org_id,
        stream_type,
        stream_name,
        Some(date_range),
        &LOCAL_NODE.uuid.clone(),
    )
    .await;
    // already bind to this node, we can unlock now
    dist_lock::unlock(&locker).await?;
    drop(locker);
    ret?;

    // same date, just mark delete done
    if date_range.0 == date_range.1 {
        // mark delete done
        return db::compact::retention::delete_stream_done(
            org_id,
            stream_type,
            stream_name,
            Some(date_range),
        )
        .await;
    }

    let mut date_start =
        DateTime::parse_from_rfc3339(&format!("{}T00:00:00Z", date_range.0))?.with_timezone(&Utc);
    // Hack for 1970-01-01
    if date_range.0 == "1970-01-01" {
        date_start += Duration::try_milliseconds(1).unwrap();
    }
    let date_end =
        DateTime::parse_from_rfc3339(&format!("{}T00:00:00Z", date_range.1))?.with_timezone(&Utc);
    let time_range = {
        (
            date_start.timestamp_micros(),
            date_end.timestamp_micros() - 1,
        )
    };

    if is_local_disk_storage() {
        let dirs_to_delete =
            generate_local_dirs(org_id, stream_type, stream_name, date_start, date_end);
        // Delete all collected directories in parallel
        let mut delete_tasks = vec![];
        for dir in dirs_to_delete {
            log::info!(
                "[COMPACTOR] stream {org_id}/{stream_type}/{stream_name} delete dir: {dir:?}"
            );
            delete_tasks.push(tokio::fs::remove_dir_all(dir));
        }
        futures::future::try_join_all(delete_tasks).await?;
    }

    // delete from file list
    delete_from_file_list(org_id, stream_type, stream_name, time_range).await?;

    super::super::file_list_dump::delete_in_time_range(
        org_id,
        stream_type,
        stream_name,
        (date_start.timestamp_micros(), date_end.timestamp_micros()),
    )
    .await?;

    // archive old schema versions
    let mut schema_versions =
        infra::schema::get_versions(org_id, stream_name, stream_type, Some(time_range)).await?;
    // pop last version, it's the current version
    schema_versions.pop();
    for schema in schema_versions {
        let start_dt: i64 = match schema.metadata().get("start_dt") {
            Some(v) => v.parse().unwrap_or_default(),
            None => 0,
        };
        if start_dt == 0 {
            continue;
        }
        infra::schema::history::create(org_id, stream_type, stream_name, start_dt, schema).await?;
        infra::schema::delete(org_id, stream_type, stream_name, Some(start_dt)).await?;
    }

    // update stream stats retention time
    let mut stats = cache::stats::get_stream_stats(org_id, stream_name, stream_type);
    let mut min_ts = infra_file_list::get_min_ts(org_id, stream_type, stream_name)
        .await
        .unwrap_or_default();
    if min_ts == 0 {
        min_ts = stats.doc_time_min;
    };
    infra_file_list::reset_stream_stats_min_ts(
        org_id,
        format!("{org_id}/{stream_type}/{stream_name}").as_str(),
        min_ts,
    )
    .await?;
    // update stream stats in cache
    if min_ts > stats.doc_time_min {
        stats.doc_time_min = min_ts;
        cache::stats::set_stream_stats(org_id, stream_name, stream_type, stats);
    }

    // mark delete done
    db::compact::retention::delete_stream_done(org_id, stream_type, stream_name, Some(date_range))
        .await
}

async fn delete_from_file_list(
    org_id: &str,
    stream_type: StreamType,
    stream_name: &str,
    time_range: (i64, i64),
) -> Result<(), anyhow::Error> {
    let task_id = tokio::task::try_id()
        .map(|id| id.to_string())
        .unwrap_or_else(|| rand::random::<u64>().to_string());
    let fake_trace_id = format!(
        "delete_from_file_list-{}-{}-{}",
        task_id, time_range.0, time_range.1
    );
    let files = file_list::query(
        &fake_trace_id,
        org_id,
        stream_name,
        stream_type,
        PartitionTimeLevel::Unset,
        time_range.0,
        time_range.1,
    )
    .await?;
    if files.is_empty() {
        return Ok(());
    }

    let mut hours_files: HashMap<String, Vec<FileKey>> = HashMap::with_capacity(24);
    for mut file in files {
        let columns: Vec<_> = file.key.split('/').collect();
        let hour_key = format!(
            "{}/{}/{}/{}",
            columns[4], columns[5], columns[6], columns[7]
        );
        let entry = hours_files.entry(hour_key).or_default();
        file.deleted = true;
        entry.push(file);
    }

    // write file list to storage
    write_file_list(org_id, &hours_files).await?;

    Ok(())
}

// write file list to db, all the files should be deleted
async fn write_file_list(
    org_id: &str,
    hours_files: &HashMap<String, Vec<FileKey>>,
) -> Result<(), anyhow::Error> {
    let cfg = get_config();
    for events in hours_files.values() {
        // set to db, retry 5 times
        let mut success = false;
        let created_at = Utc::now().timestamp_micros();
        for _ in 0..5 {
            // only store the file_list into history, don't delete files
            if cfg.compact.data_retention_history
                && let Err(e) = infra_file_list::batch_add_history(events).await
            {
                log::error!("[COMPACTOR] file_list batch_add_history failed: {e}");
                return Err(e.into());
            }
            // delete from file_list table
            if let Err(e) = infra_file_list::batch_process(events).await {
                log::error!("[COMPACTOR] batch_delete to db failed, retrying: {e}");
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                continue;
            }
            // store to file_list_deleted table, pending delete
            if !cfg.compact.data_retention_history {
                let del_items = events
                    .iter()
                    .map(|v| FileListDeleted {
                        id: 0,
                        account: v.account.clone(),
                        file: v.key.clone(),
                        index_file: v.meta.index_size > 0,
                        flattened: v.meta.flattened,
                    })
                    .collect::<Vec<_>>();
                if let Err(e) =
                    infra_file_list::batch_add_deleted(org_id, created_at, &del_items).await
                {
                    log::error!("[COMPACTOR] batch_add_deleted to db failed, retrying: {e}");
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            }
            success = true;
            break;
        }
        if !success {
            return Err(anyhow::anyhow!("[COMPACTOR] batch_write to db failed"));
        }
    }
    Ok(())
}

fn generate_local_dirs(
    org_id: &str,
    stream_type: StreamType,
    stream_name: &str,
    mut date_start: DateTime<Utc>,
    date_end: DateTime<Utc>,
) -> Vec<PathBuf> {
    let cfg = get_config();
    let mut dirs_to_delete = Vec::new();
    while date_start <= date_end {
        let day_dir = format!(
            "{}files/{org_id}/{stream_type}/{stream_name}/{}",
            cfg.common.data_stream_dir,
            date_start.format("%Y/%m/%d")
        );
        let day_path = std::path::Path::new(&day_dir);
        if day_path.exists() {
            dirs_to_delete.push(day_path.to_path_buf());
        }
        // index data
        let day_dir = format!(
            "{}files/{org_id}/index/{stream_name}_{stream_type}/{}",
            cfg.common.data_stream_dir,
            date_start.format("%Y/%m/%d")
        );
        let day_path = std::path::Path::new(&day_dir);
        if day_path.exists() {
            dirs_to_delete.push(day_path.to_path_buf());
        }
        date_start += Duration::days(1); // Move to the next day
    }

    dirs_to_delete
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;

    use super::*;

    #[tokio::test]
    async fn test_delete_by_stream() {
        infra_file_list::create_table().await.unwrap();
        let org_id = "test";
        let stream_name = "test";
        let stream_type = config::meta::stream::StreamType::Logs;
        let lifecycle_end = DateTime::parse_from_rfc3339("2023-01-01T00:00:00Z")
            .unwrap()
            .to_utc();
        let res = delete_by_stream(&lifecycle_end, org_id, stream_type, stream_name, &[]).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_delete_all() {
        infra_file_list::create_table().await.unwrap();
        let org_id = "test";
        let stream_name = "test";
        let stream_type = config::meta::stream::StreamType::Logs;
        let res = delete_all(org_id, stream_type, stream_name).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_populate_time_ranges() {
        let now = Utc::now();
        let exclude_range = TimeRange::new(
            (now - Duration::try_days(2).unwrap()).timestamp_micros(),
            (now - Duration::try_days(1).unwrap()).timestamp_micros(),
        );
        let original_time_range = TimeRange::new(
            (now - Duration::try_days(2).unwrap()).timestamp_micros(),
            now.timestamp_micros(),
        );
        let last_retained_time = (now - Duration::try_days(5).unwrap()).timestamp_micros();
        println!("original time range : {original_time_range}");
        println!("red day time range : {exclude_range}");
        let time_ranges_to_delete = generate_time_ranges_for_deletion(
            vec![exclude_range],
            original_time_range.clone(),
            last_retained_time,
        );
        assert_eq!(time_ranges_to_delete.len(), 1);
        println!(
            "res time ranges : {}",
            time_ranges_to_delete.iter().join(", ")
        );
    }

    #[tokio::test]
    async fn test_populate_time_ranges_contains() {
        let now = Utc::now();
        let original_time_range = TimeRange::new(
            (now - Duration::try_days(15).unwrap()).timestamp_micros(),
            now.timestamp_micros(),
        );
        let exclude_range = TimeRange::new(
            (now - Duration::try_days(3).unwrap()).timestamp_micros(),
            (now - Duration::try_days(2).unwrap()).timestamp_micros(),
        );
        let last_retained_time = (now - Duration::try_days(5).unwrap()).timestamp_micros();
        println!("original time range : {original_time_range}");
        println!("red day time range : {exclude_range}");
        let res_time_ranges = generate_time_ranges_for_deletion(
            vec![exclude_range],
            original_time_range.clone(),
            last_retained_time,
        );
        assert_eq!(res_time_ranges.len(), 2);
        println!("res time ranges : {}", res_time_ranges.iter().join(", "));
        assert!(res_time_ranges[0].intersects(&original_time_range));
    }

    #[tokio::test]
    async fn test_populate_time_ranges_intersecting_ext_ret_days() {
        let now = Utc::now();
        let exclude_range_1 = TimeRange::new(
            (now - Duration::try_days(20).unwrap()).timestamp_micros(),
            (now - Duration::try_days(10).unwrap()).timestamp_micros(),
        );
        let exclude_range_2 = TimeRange::new(
            (now - Duration::try_days(15).unwrap()).timestamp_micros(),
            (now - Duration::try_days(5).unwrap()).timestamp_micros(),
        );

        let time_range =
            TimeRange::flatten_overlapping_ranges(vec![exclude_range_1, exclude_range_2]);

        assert_eq!(time_range.len(), 1);
    }

    #[tokio::test]
    async fn test_populate_time_ranges_out_of_last_retained_period() {
        let now = Utc::now();
        let exclude_ranges = vec![
            TimeRange::new(
                (now - Duration::try_days(20).unwrap()).timestamp_micros(),
                (now - Duration::try_days(10).unwrap()).timestamp_micros(),
            ),
            TimeRange::new(
                (now - Duration::try_days(8).unwrap()).timestamp_micros(),
                (now - Duration::try_days(5).unwrap()).timestamp_micros(),
            ),
        ];

        let original_time_range = TimeRange::new(
            (now - Duration::try_days(30).unwrap()).timestamp_micros(),
            now.timestamp_micros(),
        );
        let last_retained_time = (now - Duration::try_days(5).unwrap()).timestamp_micros();
        let res_time_ranges = generate_time_ranges_for_deletion(
            exclude_ranges,
            original_time_range.clone(),
            last_retained_time,
        );

        println!("original time range : {original_time_range}");
        println!("res time ranges : {}", res_time_ranges.iter().join(", "));
        assert_eq!(res_time_ranges.len(), 2);
    }
}
