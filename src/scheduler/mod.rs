//! Scheduled bookings: fire a booking the instant a tee sheet opens.
//!
//! A dispatcher loop polls for jobs whose firing time is near and hands each to
//! a dedicated task that pre-authenticates early, sleeps until the exact moment,
//! then books with rapid retries. Times are stored in UTC; the web layer
//! converts to/from each club's local timezone.

use crate::email::Mailer;
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
/// A job armed more than this long after its target was almost certainly missed
/// during an outage — fail it instead of racing a long-opened sheet. This sits
/// well beyond the rapid-retry envelope (a few re-arms span well under a minute),
/// so normal retries are never mistaken for stale jobs.
const STALE_GRACE_SECS: i64 = 600;

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
    mailer: Mailer,
    /// Public base URL for links in notification emails.
    base_url: String,
}

impl JobScheduler {
    pub fn new(db: SqlitePool, dry_run: bool, mailer: Mailer, base_url: String) -> Self {
        Self {
            db,
            dry_run,
            mailer,
            base_url,
        }
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
        let mailer = self.mailer.clone();
        let base_url = self.base_url.clone();
        tokio::spawn(async move {
            info!(dry_run, "job scheduler started");
            loop {
                if let Err(e) = dispatch_due_jobs(&db, dry_run, &mailer, &base_url).await {
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
    if let Err(e) = set_status(db, id, JobStatus::Failed, Some(&msg)).await {
        error!(
            job_id = id,
            "additionally failed to persist failed status: {e}"
        );
    }
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
async fn dispatch_due_jobs(
    db: &SqlitePool,
    dry_run: bool,
    mailer: &Mailer,
    base_url: &str,
) -> Result<(), sqlx::Error> {
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
        let mailer = mailer.clone();
        let base_url = base_url.to_string();
        let job_id = job.id;
        tokio::spawn(async move {
            // Run the job in a nested task and await its handle: if its body
            // panics, crash recovery only re-queues `running` rows at startup —
            // which a long-lived process may never reach — so the row would be
            // stranded `running` forever. Catch the panic here and fail the row.
            let inner = {
                let db = db.clone();
                let mailer = mailer.clone();
                let base_url = base_url.clone();
                tokio::spawn(
                    async move { arm_and_run_job(&db, dry_run, &mailer, &base_url, job).await },
                )
            };
            if let Err(join_err) = inner.await {
                if join_err.is_panic() {
                    fail_if_running(&db, job_id, "job task panicked").await;
                }
            }
        });
    }
    Ok(())
}

/// Mark a job failed *only if it is still `running`*, so a panic recovered after
/// the job already finished (completed/failed) doesn't clobber its real outcome.
async fn fail_if_running(db: &SqlitePool, job_id: i64, msg: &str) {
    error!(job_id, "{msg}");
    let result = sqlx::query(
        "UPDATE scheduled_jobs SET status = 'failed', last_error = ? \
         WHERE id = ? AND status = 'running'",
    )
    .bind(msg)
    .bind(job_id)
    .execute(db)
    .await;
    if let Err(e) = result {
        error!(job_id, "failed to mark panicked job failed: {e}");
    }
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
async fn arm_and_run_job(
    db: &SqlitePool,
    dry_run: bool,
    mailer: &Mailer,
    base_url: &str,
    job: ScheduledJob,
) {
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
    let (client, club_name) = match crate::clubs::get(db, club_id).await {
        Ok(Some(club)) => (GolfClient::from_club(&club), club.name),
        Ok(None) => return fail_job(db, job.id, format!("club {club_id} not found")).await,
        Err(e) => return fail_job(db, job.id, format!("failed to load club {club_id}: {e}")).await,
    };

    // A job whose target is far in the past was missed (e.g. the process was
    // down when it should have fired). Firing now would race a sheet that opened
    // long ago — book nothing useful, or the wrong thing — so fail it and notify
    // rather than fire late. The target is stable across retries, so a job being
    // rapidly retried near its deadline is never caught here.
    let lateness = Utc::now() - target;
    if lateness > chrono::Duration::seconds(STALE_GRACE_SECS) {
        let msg = format!(
            "missed scheduled time by {}s (the server was likely down when it should have fired)",
            lateness.num_seconds()
        );
        if let Err(e) = set_status(db, job.id, JobStatus::Failed, Some(&msg)).await {
            error!(job_id = job.id, "failed to mark stale job failed: {e}");
        }
        notify(
            db,
            mailer,
            base_url,
            &job,
            &club_name,
            &booking,
            dry_run,
            Err(msg),
        )
        .await;
        return;
    }

    info!(job_id = job.id, %target, "arming job");

    match fire_booking(&client, dry_run, &booking, target).await {
        Ok(()) => {
            info!(job_id = job.id, "job completed");
            // A successful booking whose status fails to persist stays `running`,
            // and crash recovery would later re-fire it (a double-book) — so a
            // failed write here must be loud, not silent.
            if let Err(e) = set_status(db, job.id, JobStatus::Completed, None).await {
                error!(
                    job_id = job.id,
                    "booking succeeded but marking completed failed: {e}"
                );
            }
            if let Err(e) = mark_completed(db, job.id).await {
                error!(
                    job_id = job.id,
                    "booking succeeded but setting completed_at failed: {e}"
                );
            }
            notify(
                db,
                mailer,
                base_url,
                &job,
                &club_name,
                &booking,
                dry_run,
                Ok(()),
            )
            .await;
        }
        Err(e) => {
            let msg = e.message().to_string();
            if should_requeue(e.is_terminal(), job.attempts, job.max_attempts) {
                // Back to pending so the dispatcher re-arms it on a later tick.
                // No email yet — it'll retry.
                if let Err(db_err) = increment_attempts(db, job.id, &msg).await {
                    error!(job_id = job.id, "failed to requeue job for retry: {db_err}");
                }
            } else {
                // Terminal failure, or out of attempts: record it and notify.
                if let Err(db_err) = set_status(db, job.id, JobStatus::Failed, Some(&msg)).await {
                    error!(job_id = job.id, "failed to mark job failed: {db_err}");
                }
                notify(
                    db,
                    mailer,
                    base_url,
                    &job,
                    &club_name,
                    &booking,
                    dry_run,
                    Err(msg),
                )
                .await;
            }
        }
    }
}

/// Email the scheduling user about a final booking outcome, if they have an
/// address and the mailer is enabled.
#[allow(clippy::too_many_arguments)]
async fn notify(
    db: &SqlitePool,
    mailer: &Mailer,
    base_url: &str,
    job: &ScheduledJob,
    club_name: &str,
    booking: &BookingJobData,
    dry_run: bool,
    outcome: Result<(), String>,
) {
    let email = match crate::users::email_for(db, job.user_id).await {
        Ok(Some(e)) => e,
        Ok(None) => return,
        Err(e) => {
            warn!(
                "notify: failed to look up email for user {}: {e}",
                job.user_id
            );
            return;
        }
    };

    let prefix = if dry_run { "[DRY RUN] " } else { "" };
    let (subject, detail) = match &outcome {
        Ok(()) if dry_run => (
            format!("{prefix}Would have booked at {club_name}"),
            "Dry run — no real booking was made.".to_string(),
        ),
        Ok(()) => (
            format!("✅ Booked at {club_name}"),
            "Your scheduled booking went through.".to_string(),
        ),
        Err(reason) => (
            format!("{prefix}❌ Booking failed at {club_name}"),
            format!("Your scheduled booking did not go through.\nReason: {reason}"),
        ),
    };

    let body = format!(
        "{detail}\n\nClub: {club_name}\nEvent: {}\nBooking group: {}\n\nView your bookings: {base_url}/scheduled-jobs\n",
        booking.event_id, booking.booking_group_id,
    );

    match mailer.send(&email, &subject, body).await {
        Ok(true) => info!(job_id = job.id, "sent booking notification"),
        Ok(false) => {} // mailer disabled
        Err(e) => warn!(job_id = job.id, "failed to send booking notification: {e}"),
    }
}

/// Why a firing attempt ended without a booking, carrying whether re-arming the
/// job could plausibly change the outcome.
enum FireError {
    /// Permanent: re-arming won't help (already booked, ineligible).
    Terminal(String),
    /// Transient: worth re-arming on a later tick (retry window elapsed, a
    /// login/network hiccup).
    Retryable(String),
}

impl FireError {
    fn message(&self) -> &str {
        match self {
            FireError::Terminal(m) | FireError::Retryable(m) => m,
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, FireError::Terminal(_))
    }
}

/// Decide whether a failed firing should be re-armed for another attempt.
/// Terminal failures never re-arm; retryable ones re-arm until the attempt
/// budget is spent. Pure so the re-arm policy can be unit-tested.
fn should_requeue(is_terminal: bool, attempts: i64, max_attempts: i64) -> bool {
    !is_terminal && attempts + 1 < max_attempts
}

/// Pre-authenticate, sleep until the exact firing moment, then book with rapid
/// retry. In dry-run the network calls are replaced with logs but the real
/// timing still happens, so the firing path is exercised safely.
async fn fire_booking(
    client: &GolfClient,
    dry_run: bool,
    booking: &BookingJobData,
    target: DateTime<Utc>,
) -> Result<(), FireError> {
    let group = booking.booking_group_id;

    // 1. Sleep to the pre-auth point and log in (fresh cookies before the race).
    sleep_until(target, PREAUTH_LEAD_SECS).await;
    if dry_run {
        info!("[DRY RUN] would log in (pre-auth) for group {group}");
    } else {
        // A pre-auth hiccup is transient — let the job re-arm and try again.
        client
            .login()
            .await
            .map_err(|e| FireError::Retryable(format!("pre-auth login failed: {e}")))?;
    }

    // 2. Sleep to the exact firing moment.
    sleep_until(target, 0).await;

    // 3. Fire, retrying rapidly until the retry window elapses.
    let deadline = Utc::now() + chrono::Duration::milliseconds(RETRY_WINDOW.as_millis() as i64);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        if dry_run {
            info!("[DRY RUN] would book group {group} [attempt {attempt}]");
            return Ok(());
        }
        match client.book(group).await {
            Ok(()) => return Ok(()),
            // Terminal errors (already booked, ineligible) won't change on retry —
            // surface that so the job fails now instead of being re-armed in vain.
            Err(e) if !e.is_retryable() => return Err(FireError::Terminal(e.to_string())),
            Err(e) => {
                if Utc::now() >= deadline {
                    // The window elapsed without success; the sheet may open a touch
                    // late, so this is worth re-arming for another burst.
                    return Err(FireError::Retryable(format!(
                        "{e} (gave up after {attempt} attempt(s))"
                    )));
                }
                warn!("booking attempt {attempt} for group {group} failed: {e} — retrying");
                sleep(RETRY_INTERVAL).await;
            }
        }
    }
}

/// Sleep until `target - lead_secs`, returning immediately if already past it.
async fn sleep_until(target: DateTime<Utc>, lead_secs: i64) {
    if let Some(delay) = delay_until(target, lead_secs, Utc::now()) {
        sleep(delay).await;
    }
}

/// How long to wait from `now` until `target - lead_secs`, or `None` if that
/// moment has already passed (a negative span yields no wait).
fn delay_until(target: DateTime<Utc>, lead_secs: i64, now: DateTime<Utc>) -> Option<Duration> {
    let fire_at = target - chrono::Duration::seconds(lead_secs);
    (fire_at - now).to_std().ok()
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
    fn delay_until_waits_for_future_preauth_point() {
        let now = DateTime::parse_from_rfc3339("2026-06-20T00:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let target = now + chrono::Duration::seconds(100);
        // Pre-auth 30s early -> fire point is 70s away.
        let delay = delay_until(target, PREAUTH_LEAD_SECS, now).unwrap();
        assert_eq!(delay.as_secs(), 70);
    }

    #[test]
    fn delay_until_is_none_when_past() {
        let now = DateTime::parse_from_rfc3339("2026-06-20T00:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        // Target already 10s ago -> no wait.
        let target = now - chrono::Duration::seconds(10);
        assert!(delay_until(target, 0, now).is_none());
        // Within the pre-auth lead of the target -> also already due.
        let soon = now + chrono::Duration::seconds(10);
        assert!(delay_until(soon, PREAUTH_LEAD_SECS, now).is_none());
    }

    #[test]
    fn requeues_retryable_until_attempts_exhausted() {
        // Retryable failure with attempts to spare -> re-arm.
        assert!(should_requeue(false, 0, 3));
        assert!(should_requeue(false, 1, 3));
        // Final attempt used -> stop re-arming.
        assert!(!should_requeue(false, 2, 3));
        // Terminal failure -> never re-arm, even with attempts left.
        assert!(!should_requeue(true, 0, 3));
        // A single-shot budget fires once and stops.
        assert!(!should_requeue(false, 0, 1));
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

    // --- DB-backed tests -------------------------------------------------

    use crate::email::Mailer;
    use crate::test_support::{seed_club, seed_user, test_pool};

    /// A scheduler over an in-memory DB (dry-run, mailer disabled), plus a
    /// seeded user and club to satisfy the foreign keys.
    async fn fixture() -> (JobScheduler, i64, i64) {
        let db = test_pool().await;
        let user_id = seed_user(&db, "alice").await;
        let club_id = seed_club(&db, "Ridge").await;
        let mailer = Mailer::from_config(None).unwrap();
        let scheduler = JobScheduler::new(db, true, mailer, "http://localhost".to_string());
        (scheduler, user_id, club_id)
    }

    fn future() -> DateTime<Utc> {
        Utc::now() + chrono::Duration::hours(1)
    }

    #[tokio::test]
    async fn schedule_then_list_returns_the_job() {
        let (sched, user_id, club_id) = fixture().await;
        let id = sched
            .schedule_booking(user_id, club_id, 101, 5001, future())
            .await
            .unwrap();

        let jobs = sched.get_user_jobs(user_id).await.unwrap();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.id, id);
        assert_eq!(job.status, "pending");
        assert_eq!(job.club_id, Some(club_id));
        assert_eq!(job.event_id, Some(101));
        assert_eq!(job.max_attempts, 3);
        assert_eq!(job.attempts, 0);
        // job_data deserializes back to the booking parameters.
        let JobData::Booking(b) = job.job_data().unwrap();
        assert_eq!(b.booking_group_id, 5001);
    }

    #[tokio::test]
    async fn user_only_sees_their_own_jobs() {
        let (sched, alice, club_id) = fixture().await;
        let bob = seed_user(&sched.db, "bob").await;
        sched
            .schedule_booking(alice, club_id, 1, 1, future())
            .await
            .unwrap();
        sched
            .schedule_booking(bob, club_id, 2, 2, future())
            .await
            .unwrap();

        assert_eq!(sched.get_user_jobs(alice).await.unwrap().len(), 1);
        assert_eq!(sched.get_user_jobs(bob).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_only_affects_owners_pending_job() {
        let (sched, alice, club_id) = fixture().await;
        let bob = seed_user(&sched.db, "bob").await;
        let id = sched
            .schedule_booking(alice, club_id, 1, 1, future())
            .await
            .unwrap();

        // Wrong owner can't cancel.
        assert!(!sched.cancel_job(bob, id).await.unwrap());
        // Owner cancels; row flips to cancelled.
        assert!(sched.cancel_job(alice, id).await.unwrap());
        assert_eq!(
            sched.get_user_jobs(alice).await.unwrap()[0].status,
            "cancelled"
        );
        // A second cancel is a no-op (no longer pending).
        assert!(!sched.cancel_job(alice, id).await.unwrap());
    }

    #[tokio::test]
    async fn claim_job_is_atomic() {
        let (sched, user_id, club_id) = fixture().await;
        let id = sched
            .schedule_booking(user_id, club_id, 1, 1, future())
            .await
            .unwrap();

        // First claim wins (pending -> running); second finds it already running.
        assert!(claim_job(&sched.db, id).await.unwrap());
        assert!(!claim_job(&sched.db, id).await.unwrap());
        assert_eq!(
            sched.get_user_jobs(user_id).await.unwrap()[0].status,
            "running"
        );
    }

    #[tokio::test]
    async fn requeue_stranded_resets_running_to_pending() {
        let (sched, user_id, club_id) = fixture().await;
        let id = sched
            .schedule_booking(user_id, club_id, 1, 1, future())
            .await
            .unwrap();
        claim_job(&sched.db, id).await.unwrap();

        requeue_stranded_jobs(&sched.db).await.unwrap();
        assert_eq!(
            sched.get_user_jobs(user_id).await.unwrap()[0].status,
            "pending"
        );
    }

    #[tokio::test]
    async fn increment_attempts_bumps_count_and_requeues() {
        let (sched, user_id, club_id) = fixture().await;
        let id = sched
            .schedule_booking(user_id, club_id, 1, 1, future())
            .await
            .unwrap();
        claim_job(&sched.db, id).await.unwrap();

        increment_attempts(&sched.db, id, "sheet not open yet")
            .await
            .unwrap();
        let job = &sched.get_user_jobs(user_id).await.unwrap()[0];
        assert_eq!(job.attempts, 1);
        assert_eq!(job.status, "pending");
        assert_eq!(job.last_error.as_deref(), Some("sheet not open yet"));
    }

    #[tokio::test]
    async fn set_status_and_mark_completed_persist() {
        let (sched, user_id, club_id) = fixture().await;
        let id = sched
            .schedule_booking(user_id, club_id, 1, 1, future())
            .await
            .unwrap();

        set_status(&sched.db, id, JobStatus::Failed, Some("boom"))
            .await
            .unwrap();
        let job = &sched.get_user_jobs(user_id).await.unwrap()[0];
        assert_eq!(job.status, "failed");
        assert_eq!(job.last_error.as_deref(), Some("boom"));
        assert!(job.completed_at.is_none());

        mark_completed(&sched.db, id).await.unwrap();
        assert!(sched.get_user_jobs(user_id).await.unwrap()[0]
            .completed_at
            .is_some());
    }

    #[tokio::test]
    async fn stale_job_is_failed_not_fired() {
        let (sched, user_id, club_id) = fixture().await;
        // Target is well beyond the stale grace (as if missed during an outage).
        let stale = Utc::now() - chrono::Duration::seconds(STALE_GRACE_SECS + 60);
        let id = sched
            .schedule_booking(user_id, club_id, 1, 1, stale)
            .await
            .unwrap();
        claim_job(&sched.db, id).await.unwrap();
        let job = sched.get_user_jobs(user_id).await.unwrap().pop().unwrap();

        arm_and_run_job(&sched.db, true, &sched.mailer, &sched.base_url, job).await;

        let after = &sched.get_user_jobs(user_id).await.unwrap()[0];
        assert_eq!(after.status, "failed");
        assert!(
            after
                .last_error
                .as_deref()
                .unwrap()
                .contains("missed scheduled time"),
            "got: {:?}",
            after.last_error
        );
    }

    #[tokio::test]
    async fn fresh_dry_run_job_completes_rather_than_being_called_stale() {
        // A job at (about) its target is within grace and fires normally; in
        // dry-run that means it completes. Guards against the staleness check
        // being too aggressive.
        let (sched, user_id, club_id) = fixture().await;
        let id = sched
            .schedule_booking(user_id, club_id, 1, 1, Utc::now())
            .await
            .unwrap();
        claim_job(&sched.db, id).await.unwrap();
        let job = sched.get_user_jobs(user_id).await.unwrap().pop().unwrap();

        arm_and_run_job(&sched.db, true, &sched.mailer, &sched.base_url, job).await;

        assert_eq!(
            sched.get_user_jobs(user_id).await.unwrap()[0].status,
            "completed"
        );
    }

    #[tokio::test]
    async fn fail_if_running_only_fails_running_jobs() {
        let (sched, user_id, club_id) = fixture().await;
        let id = sched
            .schedule_booking(user_id, club_id, 1, 1, future())
            .await
            .unwrap();
        claim_job(&sched.db, id).await.unwrap();

        // Running -> failed with the panic message.
        fail_if_running(&sched.db, id, "job task panicked").await;
        let job = &sched.get_user_jobs(user_id).await.unwrap()[0];
        assert_eq!(job.status, "failed");
        assert_eq!(job.last_error.as_deref(), Some("job task panicked"));
    }

    #[tokio::test]
    async fn fail_if_running_does_not_clobber_completed_jobs() {
        let (sched, user_id, club_id) = fixture().await;
        let id = sched
            .schedule_booking(user_id, club_id, 1, 1, future())
            .await
            .unwrap();
        set_status(&sched.db, id, JobStatus::Completed, None)
            .await
            .unwrap();

        // A panic recovered after the job already completed must not overwrite it.
        fail_if_running(&sched.db, id, "job task panicked").await;
        assert_eq!(
            sched.get_user_jobs(user_id).await.unwrap()[0].status,
            "completed"
        );
    }

    #[tokio::test]
    async fn deleting_a_club_nulls_the_jobs_club_id() {
        // ON DELETE SET NULL: a scheduled job survives its club being removed,
        // and the scheduler later fails it with "club was removed".
        let (sched, user_id, club_id) = fixture().await;
        sched
            .schedule_booking(user_id, club_id, 1, 1, future())
            .await
            .unwrap();

        crate::clubs::delete(&sched.db, club_id).await.unwrap();
        assert_eq!(sched.get_user_jobs(user_id).await.unwrap()[0].club_id, None);
    }
}
