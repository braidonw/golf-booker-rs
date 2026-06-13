//! Scheduled bookings: fire a booking the instant a tee sheet opens.
//!
//! A dispatcher loop polls for jobs whose firing time is near and hands each to
//! a dedicated task that pre-authenticates early, sleeps until the exact moment,
//! then books with rapid retries. Times are stored in UTC; the web layer
//! converts to/from each club's local timezone.

use crate::golf::GolfClient;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use std::str::FromStr;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

/// How often the dispatcher wakes to look for jobs to arm.
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Arm any pending job firing within this look-ahead window, so its dedicated
/// task can sleep until the exact moment.
const ARM_WINDOW_SECS: i64 = 120;
/// How long before the firing moment to log in, so the hot path is just the
/// booking POST with fresh cookies already in hand.
const PREAUTH_LEAD_SECS: i64 = 30;
/// After firing, keep retrying until this much time past the target has elapsed.
const RETRY_WINDOW: Duration = Duration::from_secs(5);
/// Delay between rapid-retry attempts within the retry window.
const RETRY_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            JobStatus::Pending => "pending",
            JobStatus::Running => "running",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        };
        f.write_str(s)
    }
}

impl FromStr for JobStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(JobStatus::Pending),
            "running" => Ok(JobStatus::Running),
            "completed" => Ok(JobStatus::Completed),
            "failed" => Ok(JobStatus::Failed),
            "cancelled" => Ok(JobStatus::Cancelled),
            _ => Err(format!("invalid job status: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookingJobData {
    pub event_id: i64,
    /// The booking group / `rowId` to book against.
    pub booking_group_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum JobData {
    #[serde(rename = "booking")]
    Booking(BookingJobData),
}

/// Full `scheduled_jobs` row. Some columns (timestamps, job_type) are mapped for
/// completeness but not yet surfaced in the UI.
#[allow(dead_code)]
#[derive(Debug, Clone, FromRow)]
pub struct ScheduledJob {
    pub id: i64,
    pub user_id: i64,
    pub club_id: Option<i64>,
    pub event_id: Option<i64>,
    pub job_type: String,
    pub scheduled_time: String,
    pub status: String,
    pub job_data: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

impl ScheduledJob {
    pub fn scheduled_time(&self) -> Result<DateTime<Utc>, chrono::ParseError> {
        DateTime::parse_from_rfc3339(&self.scheduled_time).map(|dt| dt.with_timezone(&Utc))
    }

    pub fn job_data(&self) -> Result<JobData, serde_json::Error> {
        serde_json::from_str(&self.job_data)
    }
}

/// Owns the database handle and dispatches scheduled jobs. Cheap to clone
/// (shares the pool); the background loop runs independently of clones.
#[derive(Clone)]
pub struct JobScheduler {
    db: SqlitePool,
    /// When true, jobs are simulated (logged) instead of hitting the club.
    dry_run: bool,
}

impl JobScheduler {
    pub fn new(db: SqlitePool, dry_run: bool) -> Self {
        Self { db, dry_run }
    }

    /// Spawn the background dispatcher loop. Returns immediately.
    pub async fn start(&self) {
        // Recover jobs left in `running` by a crashed/restarted process: this is
        // a single instance, so any `running` row has no live task — requeue it.
        if let Err(e) = requeue_stranded_jobs(&self.db).await {
            error!("failed to requeue stranded jobs: {e}");
        }

        let db = self.db.clone();
        let dry_run = self.dry_run;
        tokio::spawn(async move {
            info!(dry_run, "job scheduler started");
            loop {
                if let Err(e) = dispatch_due_jobs(&db, dry_run).await {
                    error!("error dispatching jobs: {e}");
                }
                sleep(POLL_INTERVAL).await;
            }
        });
    }

    /// Create a new scheduled booking job. `scheduled_time` is UTC.
    pub async fn schedule_booking(
        &self,
        user_id: i64,
        club_id: i64,
        event_id: i64,
        booking_group_id: u32,
        scheduled_time: DateTime<Utc>,
    ) -> anyhow::Result<i64> {
        let job_data = serde_json::to_string(&JobData::Booking(BookingJobData {
            event_id,
            booking_group_id,
        }))?;
        let scheduled = scheduled_time.to_rfc3339();

        let result = sqlx::query(
            "INSERT INTO scheduled_jobs \
             (user_id, club_id, event_id, job_type, scheduled_time, job_data, status) \
             VALUES (?, ?, ?, 'booking', ?, ?, 'pending')",
        )
        .bind(user_id)
        .bind(club_id)
        .bind(event_id)
        .bind(scheduled)
        .bind(job_data)
        .execute(&self.db)
        .await?;

        info!(user_id, club_id, "scheduled booking job");
        Ok(result.last_insert_rowid())
    }

    /// All jobs for a user, newest scheduled first.
    pub async fn get_user_jobs(&self, user_id: i64) -> Result<Vec<ScheduledJob>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM scheduled_jobs WHERE user_id = ? ORDER BY scheduled_time DESC",
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await
    }

    /// Cancel a pending job owned by `user_id`. Returns whether a row changed.
    pub async fn cancel_job(&self, user_id: i64, job_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE scheduled_jobs SET status = 'cancelled' \
             WHERE id = ? AND user_id = ? AND status = 'pending'",
        )
        .bind(job_id)
        .bind(user_id)
        .execute(&self.db)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

/// Mark a job failed with a message, logging it.
async fn fail_job(db: &SqlitePool, id: i64, msg: String) {
    error!("job {id} failed: {msg}");
    let _ = set_status(db, id, JobStatus::Failed, Some(&msg)).await;
}

/// Requeue any jobs stuck in `running` back to `pending` so the dispatcher can
/// re-arm them after a restart.
async fn requeue_stranded_jobs(db: &SqlitePool) -> Result<(), sqlx::Error> {
    let result =
        sqlx::query("UPDATE scheduled_jobs SET status = 'pending' WHERE status = 'running'")
            .execute(db)
            .await?;
    let n = result.rows_affected();
    if n > 0 {
        warn!("requeued {n} stranded running job(s) on startup");
    }
    Ok(())
}

/// Find jobs near their firing moment, claim each, and spawn a task to fire it.
/// The dispatcher never blocks on a job — it claims and hands off.
async fn dispatch_due_jobs(db: &SqlitePool, dry_run: bool) -> Result<(), sqlx::Error> {
    let arm_horizon = (Utc::now() + chrono::Duration::seconds(ARM_WINDOW_SECS)).to_rfc3339();

    let armable: Vec<ScheduledJob> = sqlx::query_as(
        "SELECT * FROM scheduled_jobs \
         WHERE status = 'pending' AND scheduled_time <= ? ORDER BY scheduled_time ASC",
    )
    .bind(arm_horizon)
    .fetch_all(db)
    .await?;

    for job in armable {
        // Atomically claim (pending -> running) so we never double-arm a job.
        if !claim_job(db, job.id).await? {
            continue;
        }
        let db = db.clone();
        tokio::spawn(async move {
            arm_and_run_job(&db, dry_run, job).await;
        });
    }
    Ok(())
}

/// Atomically transition a job from `pending` to `running`. Returns whether this
/// caller won the claim.
async fn claim_job(db: &SqlitePool, job_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE scheduled_jobs SET status = 'running' WHERE id = ? AND status = 'pending'",
    )
    .bind(job_id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Arm a claimed job: resolve its club, pre-authenticate, sleep to the moment,
/// fire with rapid retry, and record the outcome.
async fn arm_and_run_job(db: &SqlitePool, dry_run: bool, job: ScheduledJob) {
    let target = match job.scheduled_time() {
        Ok(t) => t,
        Err(e) => return fail_job(db, job.id, format!("invalid scheduled_time: {e}")).await,
    };

    let booking = match job.job_data() {
        Ok(JobData::Booking(d)) => d,
        Err(e) => return fail_job(db, job.id, format!("invalid job_data: {e}")).await,
    };

    let Some(club_id) = job.club_id else {
        return fail_job(db, job.id, "club was removed".to_string()).await;
    };
    let client = match crate::clubs::get(db, club_id).await {
        Ok(Some(club)) => GolfClient::from_club(&club),
        Ok(None) => return fail_job(db, job.id, format!("club {club_id} not found")).await,
        Err(e) => return fail_job(db, job.id, format!("failed to load club {club_id}: {e}")).await,
    };

    info!(job_id = job.id, %target, "arming job");

    match fire_booking(&client, dry_run, &booking, target).await {
        Ok(()) => {
            info!("job {} completed", job.id);
            let _ = set_status(db, job.id, JobStatus::Completed, None).await;
            let _ = mark_completed(db, job.id).await;
        }
        Err(e) => {
            let msg = e.to_string();
            if job.attempts + 1 >= job.max_attempts {
                let _ = set_status(db, job.id, JobStatus::Failed, Some(&msg)).await;
            } else {
                // Back to pending so the dispatcher re-arms it on a later tick.
                let _ = increment_attempts(db, job.id, &msg).await;
            }
        }
    }
}

/// Pre-authenticate, sleep until the exact firing moment, then book with rapid
/// retry. In dry-run the network calls are replaced with logs but the real
/// timing still happens, so the firing path is exercised safely.
async fn fire_booking(
    client: &GolfClient,
    dry_run: bool,
    booking: &BookingJobData,
    target: DateTime<Utc>,
) -> anyhow::Result<()> {
    let group = booking.booking_group_id;

    // 1. Sleep to the pre-auth point and log in (fresh cookies before the race).
    sleep_until(target, PREAUTH_LEAD_SECS).await;
    if dry_run {
        info!("[DRY RUN] would log in (pre-auth) for group {group}");
    } else {
        client.login().await?;
    }

    // 2. Sleep to the exact firing moment.
    sleep_until(target, 0).await;

    // 3. Fire, retrying rapidly until the retry window elapses.
    let deadline = Utc::now() + chrono::Duration::from_std(RETRY_WINDOW)?;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        if dry_run {
            info!("[DRY RUN] would book group {group} [attempt {attempt}]");
            return Ok(());
        }
        match client.book(group).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                // Retry on any error until the deadline. Classifying retryable
                // ("sheet not open yet") vs terminal ("already booked") needs
                // real MiClub responses — a Phase 6 follow-up.
                if Utc::now() >= deadline {
                    return Err(e.context(format!("gave up after {attempt} attempt(s)")));
                }
                warn!("booking attempt {attempt} for group {group} failed: {e} — retrying");
                sleep(RETRY_INTERVAL).await;
            }
        }
    }
}

/// Sleep until `target - lead_secs`, returning immediately if already past it.
async fn sleep_until(target: DateTime<Utc>, lead_secs: i64) {
    let fire_at = target - chrono::Duration::seconds(lead_secs);
    if let Ok(delta) = (fire_at - Utc::now()).to_std() {
        sleep(delta).await;
    }
    // A negative delta -> to_std() errors -> we're already due, fire now.
}

async fn set_status(
    db: &SqlitePool,
    job_id: i64,
    status: JobStatus,
    error_msg: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE scheduled_jobs SET status = ?, last_error = ? WHERE id = ?")
        .bind(status.to_string())
        .bind(error_msg)
        .bind(job_id)
        .execute(db)
        .await?;
    Ok(())
}

async fn mark_completed(db: &SqlitePool, job_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE scheduled_jobs SET completed_at = ? WHERE id = ?")
        .bind(Utc::now().to_rfc3339())
        .bind(job_id)
        .execute(db)
        .await?;
    Ok(())
}

async fn increment_attempts(
    db: &SqlitePool,
    job_id: i64,
    error_msg: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE scheduled_jobs SET attempts = attempts + 1, status = 'pending', last_error = ? \
         WHERE id = ?",
    )
    .bind(error_msg)
    .bind(job_id)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_status_roundtrips() {
        for s in [
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Completed,
            JobStatus::Failed,
            JobStatus::Cancelled,
        ] {
            assert_eq!(s.to_string().parse::<JobStatus>().unwrap(), s);
        }
        assert!("bogus".parse::<JobStatus>().is_err());
    }

    #[test]
    fn job_data_roundtrips_as_tagged_json() {
        let data = JobData::Booking(BookingJobData {
            event_id: 101,
            booking_group_id: 5001,
        });
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("\"type\":\"booking\""));
        let back: JobData = serde_json::from_str(&json).unwrap();
        let JobData::Booking(b) = back;
        assert_eq!(b.event_id, 101);
        assert_eq!(b.booking_group_id, 5001);
    }
}
