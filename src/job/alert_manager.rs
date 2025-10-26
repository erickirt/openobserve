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

use config::{cluster::LOCAL_NODE, get_config, spawn_pausable_job};
#[cfg(feature = "enterprise")]
use o2_enterprise::enterprise::common::config::get_config as get_o2_config;

use crate::service;

pub async fn run() -> Result<(), anyhow::Error> {
    if !LOCAL_NODE.is_alert_manager() {
        return Ok(());
    }

    let cfg = get_config();
    if cfg.report_server.enable_report_server {
        log::info!(
            "Spawning embedded report server {}",
            cfg.report_server.enable_report_server
        );
        tokio::task::spawn(async move {
            if let Err(e) = report_server::spawn_server().await {
                log::error!("report server failed to spawn {e}");
            }
        });
    }

    // check super cluster
    #[cfg(feature = "enterprise")]
    if get_o2_config().super_cluster.enabled {
        let local_cluster_name = config::get_cluster_name();
        let exist_cluster_name =
            o2_enterprise::enterprise::super_cluster::kv::alert_manager::get_job_cluster().await?;
        if !exist_cluster_name.is_empty() && exist_cluster_name != local_cluster_name {
            let clusters =
                o2_enterprise::enterprise::super_cluster::kv::cluster::list_by_role_group(None)
                    .await?;
            if clusters.iter().any(|c| c.name == exist_cluster_name) {
                log::info!("[ALERT MANAGER] is running in cluster: {exist_cluster_name}");
                return Ok(());
            }
        }
        // register to super cluster
        o2_enterprise::enterprise::super_cluster::kv::alert_manager::register_job_cluster(
            &local_cluster_name,
        )
        .await?;
    }

    tokio::task::spawn(async move { run_schedule_jobs().await });
    spawn_pausable_job!(
        "alert_manager_watch_timeout",
        get_config().limit.scheduler_watch_interval,
        {
            if let Err(e) = infra::scheduler::watch_timeout().await {
                log::error!("[SCHEDULER] watch timeout jobs error: {e}");
            }
        }
    );
    #[cfg(feature = "enterprise")]
    for i in 0..cfg.limit.search_job_workers {
        spawn_pausable_job!(
            format!("search_job_worker_{}", i),
            get_config().limit.search_job_scheduler_interval,
            {
                if let Err(e) = service::search_jobs::run(i).await {
                    log::error!("[SEARCH JOB {i}] run search jobs error: {e}");
                }
            }
        );
    }
    #[cfg(feature = "enterprise")]
    spawn_pausable_job!(
        "search_job_check_running",
        get_config().limit.search_job_run_timeout,
        {
            log::debug!("[SEARCH JOB] Running check on running jobs");
            let now = config::utils::time::now_micros();
            let updated_at = now - (get_config().limit.search_job_run_timeout as i64 * 1_000_000);
            if let Err(e) =
                service::db::search_job::search_jobs::check_running_jobs(updated_at).await
            {
                log::error!("[SEARCH JOB] Error checking running jobs: {e}");
            }
        }
    );

    // Alert deduplication state cleanup job
    spawn_pausable_job!(
        "alert_dedup_cleanup",
        3600, // Run every hour
        {
            if let Err(e) = cleanup_alert_dedup_state().await {
                log::error!("[ALERT DEDUP CLEANUP] Error cleaning up deduplication state: {e}");
            }
        }
    );

    #[cfg(feature = "enterprise")]
    spawn_pausable_job!(
        "search_job_delete_by_retention",
        get_config().limit.search_job_retention * 24 * 60 * 60,
        {
            log::debug!("[SEARCH JOB] Running delete jobs by retention");
            let retention_seconds = get_config().limit.search_job_retention * 24 * 60 * 60;
            let now = config::utils::time::now_micros();
            let updated_at = now - (retention_seconds as i64 * 1_000_000);
            if let Err(e) = service::db::search_job::search_jobs::delete_jobs(updated_at).await {
                log::error!("[SEARCH JOB] Error deleting jobs: {e}");
            }
        }
    );
    #[cfg(feature = "enterprise")]
    spawn_pausable_job!(
        "search_job_delete",
        get_config().limit.search_job_delete_interval,
        {
            log::debug!("[SEARCH JOB] Running delete jobs");
            if let Err(e) = service::search_jobs::delete_jobs().await {
                log::error!("[SEARCH JOB] run delete jobs error: {e}");
            }
        }
    );

    Ok(())
}

/// Runs the schedule jobs
async fn run_schedule_jobs() -> Result<(), anyhow::Error> {
    service::alerts::scheduler::run().await
}

/// Cleanup old alert deduplication state records (enterprise-only feature)
#[cfg(feature = "enterprise")]
async fn cleanup_alert_dedup_state() -> Result<(), anyhow::Error> {
    log::debug!("[ALERT DEDUP CLEANUP] Starting cleanup of old deduplication state");

    // Get database connection
    let db = match infra::db::ORM_CLIENT.get() {
        Some(db) => db,
        None => {
            log::warn!("[ALERT DEDUP CLEANUP] ORM client not available, skipping cleanup");
            return Ok(());
        }
    };

    // Default cleanup: Remove records older than 24 hours
    // This is conservative - most alerts have shorter time windows
    let older_than_minutes = 24 * 60; // 24 hours

    match service::alerts::deduplication::cleanup_expired_state(db, None, older_than_minutes).await
    {
        Ok(deleted_count) => {
            if deleted_count > 0 {
                log::info!(
                    "[ALERT DEDUP CLEANUP] Cleaned up {} expired deduplication state records",
                    deleted_count
                );
            }
            Ok(())
        }
        Err(e) => {
            log::error!("[ALERT DEDUP CLEANUP] Error during cleanup: {}", e);
            Err(anyhow::anyhow!("Cleanup failed: {}", e))
        }
    }
}

/// OSS version: no-op cleanup since deduplication is enterprise-only
#[cfg(not(feature = "enterprise"))]
async fn cleanup_alert_dedup_state() -> Result<(), anyhow::Error> {
    // Deduplication is enterprise-only, nothing to clean up
    Ok(())
}
