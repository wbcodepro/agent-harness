//! What one eval attempt recorded, summaries over attempts, and paired
//! comparisons between configurations.
//!
//! The record follows the operational evaluation record of the unified
//! harness roadmap (§7). What was not observed is `null`, never zero.

use std::collections::BTreeMap;

use hame_trace::RunAccount;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The hidden checker passed and no forbidden effect was found.
    Pass,
    /// The checker failed and the agent did not claim success: it said it
    /// was blocked, or it stopped some other way.
    Fail,
    /// The agent ran out of time. The checker still ran; its verdict is in
    /// `check_passed` but the attempt does not count as a success.
    Timeout,
    /// The attempt could not be carried out: setup failed, the agent could
    /// not start, or it exited with an error and the checker failed.
    #[default]
    Error,
    /// The agent claimed success and the checker disagreed.
    FalseSuccess,
    /// The task needs a capability this runner does not have. Not run, and
    /// left out of every denominator.
    Unsupported,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Timeout => "timeout",
            Outcome::Error => "error",
            Outcome::FalseSuccess => "false_success",
            Outcome::Unsupported => "unsupported",
        }
    }
}

/// Phrases in the agent's final answer that report it did not finish.
/// English and Vietnamese, matched case-insensitively.
pub const BLOCKER_MARKERS: &[&str] = &[
    "could not",
    "couldn't",
    "unable to",
    "cannot ",
    "can't ",
    "not able to",
    "failed to",
    "i was blocked",
    "is blocked",
    "did not succeed",
    "not finished",
    "could not complete",
    "không thể",
    "chưa thể",
    "không làm được",
    "chưa hoàn thành",
    "i did not ",
    "i didn't ",
    "i have not ",
    "i haven't ",
    "not downloaded",
    "refused",
    "nothing was saved",
    "stopped there",
    "was not saved",
    "is unchanged",
    "unconfirmed",
];

/// How much of the final answer is read for a blocker: its lead, up to the
/// first blank line. Answers open with their outcome; a later "my first
/// try could not open the dialog" describes a problem that was overcome,
/// and reading it as a blocker would hide a false success.
const LEAD_CHARS: usize = 600;

fn lead(text: &str) -> &str {
    let text = text.trim_start();
    let end = text.find("\n\n").unwrap_or(text.len());
    let mut end = end.min(LEAD_CHARS);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Whether the agent claimed to have done the task.
///
/// A claim is: the agent exited normally, its last turn ended `completed`
/// (not failed, cancelled, interrupted or out of steps), and the lead of
/// its final answer (up to the first blank line) does not contain one of
/// [`BLOCKER_MARKERS`]. An agent that says "I could not make the tests
/// pass" has reported a blocker, not claimed success; one that stops
/// without a word has claimed it by stopping.
pub fn claims_success(exited_ok: bool, last_turn_completed: bool, final_text: &str) -> bool {
    let text = lead(final_text).to_lowercase();
    exited_ok && last_turn_completed && !BLOCKER_MARKERS.iter().any(|marker| text.contains(marker))
}

/// What happened in one attempt, for [`classify`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Observed {
    pub setup_ok: bool,
    pub agent_started: bool,
    pub agent_exited_ok: bool,
    pub timed_out: bool,
    pub check_passed: bool,
    pub forbidden_effects: usize,
    pub claimed_success: bool,
}

/// The outcome and, for anything but a pass, the failure category.
pub fn classify(seen: Observed) -> (Outcome, Option<&'static str>) {
    let broke = if seen.forbidden_effects > 0 {
        "forbidden_effect"
    } else {
        "check_failed"
    };
    if !seen.setup_ok {
        (Outcome::Error, Some("setup_failed"))
    } else if seen.timed_out {
        (Outcome::Timeout, Some("timeout"))
    } else if seen.check_passed && seen.forbidden_effects == 0 {
        (Outcome::Pass, None)
    } else if !seen.agent_started {
        (Outcome::Error, Some("agent_not_started"))
    } else if seen.claimed_success {
        (Outcome::FalseSuccess, Some(broke))
    } else if !seen.agent_exited_ok {
        (Outcome::Error, Some("agent_error"))
    } else {
        (Outcome::Fail, Some(broke))
    }
}

/// One attempt at one task under one configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub experiment_id: String,
    pub task_set_version: String,
    pub task_id: String,
    /// 1-based.
    pub run_index: u32,
    pub arm: String,
    pub hame_commit: Option<String>,
    pub adapter_commits: Option<BTreeMap<String, String>>,
    pub model_version: Option<String>,
    /// Hash of the system prompt the agent was given.
    pub prompt_version: Option<String>,
    pub os: String,
    pub hardware: Option<String>,
    pub app_versions: Option<BTreeMap<String, String>>,
    pub viewport: Option<String>,
    pub display_scale: Option<f64>,
    pub network_profile: Option<String>,
    /// Hash of the workspace as the agent found it.
    pub initial_state_id: Option<String>,
    pub cold_or_warm: String,
    pub enabled_features: BTreeMap<String, serde_json::Value>,
    pub channels: Vec<String>,
    /// The prompt language used.
    pub lang: String,

    pub outcome: Outcome,
    pub failure_category: Option<String>,
    pub forbidden_effects: Vec<String>,
    pub human_interventions: Option<u64>,
    pub claimed_success: bool,
    pub check_passed: bool,

    /// Preparing the workspace; not part of task time.
    pub setup_ms: f64,
    pub agent_ms: f64,
    /// The hidden checker, including restoring overlays and comparing
    /// protected files.
    pub verification_ms: f64,
    /// Verified completion latency: agent plus final verification.
    pub total_ms: f64,
    pub stage_spans: BTreeMap<String, f64>,

    pub uncached_input: Option<u64>,
    pub cached_input: Option<u64>,
    pub cache_write: Option<u64>,
    pub output: Option<u64>,
    pub reasoning_if_exposed: Option<u64>,
    pub model_calls: Option<u64>,
    pub nested_model_calls: Option<u64>,
    pub attempts: Option<u64>,
    pub failed_attempts: Option<u64>,
    pub retries: Option<u64>,
    pub usage_complete: bool,
    pub tool_calls: Option<u64>,

    pub request_bytes: Option<u64>,
    pub response_bytes: Option<u64>,
    pub image_bytes: Option<u64>,
    pub screenshots: Option<u64>,
    pub semantic_observations: Option<u64>,
    pub local_compute_time: Option<f64>,
    pub peak_memory: Option<u64>,
    /// Cost of every model attempt in the run, when all are priced.
    pub billed_cost: Option<f64>,
    pub tariff_date: Option<String>,

    /// SHA-256 of workspace files the run created or changed.
    pub artifact_hashes: BTreeMap<String, String>,
    pub receipt_ids: Vec<String>,
    pub trace_reference: Option<String>,
    pub note: String,
}

impl RunRecord {
    /// Fills the usage, byte and cost fields from a trace's totals.
    pub fn apply_account(&mut self, account: &RunAccount) {
        self.uncached_input = Some(account.uncached_input);
        self.cached_input = Some(account.cache_read);
        self.cache_write = Some(account.cache_write);
        self.output = Some(account.output);
        self.reasoning_if_exposed = account.reasoning;
        self.model_calls = Some(account.model_calls);
        self.nested_model_calls = Some(account.nested_model_calls);
        self.attempts = Some(account.attempts);
        self.failed_attempts = Some(account.failed_attempts);
        self.retries = Some(account.retries);
        self.usage_complete = account.usage_complete;
        self.tool_calls = Some(account.tool_calls);
        self.request_bytes = account.request_bytes;
        self.response_bytes = account.response_bytes;
        self.image_bytes = account.image_bytes;
        self.screenshots = account.images.map(|images| images + account.tool_images);
        self.billed_cost = account.cost_usd;
        self.tariff_date = Some(account.tariff_date.clone());
        if self.model_version.is_none() && !account.models.is_empty() {
            self.model_version = Some(account.models.join(","));
        }
        for (stage, ms) in &account.stage_ms {
            self.stage_spans.insert(stage.clone(), *ms);
        }
    }
}

/// Linear interpolation between closest ranks (the common "type 7"
/// definition). `None` for no values.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = p.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let (low, high) = (rank.floor() as usize, rank.ceil() as usize);
    Some(sorted[low] + (sorted[high] - sorted[low]) * (rank - low as f64))
}

/// Totals for one configuration over its attempts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub arm: String,
    /// Attempts that ran (unsupported tasks excluded): the denominator.
    pub attempts: u64,
    pub passes: u64,
    /// Passes over attempts, every failure, timeout and error included.
    pub strict_completion_rate: Option<f64>,
    pub false_successes: u64,
    pub fails: u64,
    pub timeouts: u64,
    pub errors: u64,
    pub unsupported: u64,
    /// Over passing attempts only, verification included.
    pub verified_latency_ms_p50: Option<f64>,
    pub verified_latency_ms_p95: Option<f64>,
    /// Over every attempt that ran, so giving up early does not look fast.
    pub all_latency_ms_p50: Option<f64>,
    pub uncached_input: u64,
    pub cached_input: u64,
    pub cache_write: u64,
    pub output: u64,
    pub model_calls: u64,
    pub nested_model_calls: u64,
    pub failed_model_attempts: u64,
    pub retries: u64,
    /// False when any attempt's usage was incomplete or unknown; the token
    /// totals are then lower bounds.
    pub usage_complete: bool,
    pub request_bytes: Option<u64>,
    pub image_bytes: Option<u64>,
    /// Every attempt's cost, when all are known.
    pub cost_usd: Option<f64>,
    /// The cost of the attempts whose cost is known.
    pub known_cost_usd: f64,
    /// Total cost of all attempts divided by verified successes; undefined
    /// with no successes.
    pub cost_per_success: Option<f64>,
    pub cost_per_attempt: Option<f64>,
}

pub fn summarize(arm: &str, records: &[&RunRecord]) -> Summary {
    let mut s = Summary {
        arm: arm.to_string(),
        usage_complete: true,
        cost_usd: Some(0.0),
        request_bytes: Some(0),
        image_bytes: Some(0),
        ..Summary::default()
    };
    let mut verified = Vec::new();
    let mut all = Vec::new();
    for record in records {
        match record.outcome {
            Outcome::Unsupported => {
                s.unsupported += 1;
                continue;
            }
            Outcome::Pass => {
                s.passes += 1;
                verified.push(record.total_ms);
            }
            Outcome::Fail => s.fails += 1,
            Outcome::Timeout => s.timeouts += 1,
            Outcome::Error => s.errors += 1,
            Outcome::FalseSuccess => s.false_successes += 1,
        }
        s.attempts += 1;
        all.push(record.total_ms);
        s.uncached_input += record.uncached_input.unwrap_or(0);
        s.cached_input += record.cached_input.unwrap_or(0);
        s.cache_write += record.cache_write.unwrap_or(0);
        s.output += record.output.unwrap_or(0);
        s.model_calls += record.model_calls.unwrap_or(0);
        s.nested_model_calls += record.nested_model_calls.unwrap_or(0);
        s.failed_model_attempts += record.failed_attempts.unwrap_or(0);
        s.retries += record.retries.unwrap_or(0);
        s.usage_complete &= record.usage_complete;
        s.request_bytes = s
            .request_bytes
            .zip(record.request_bytes)
            .map(|(a, b)| a + b);
        s.image_bytes = s.image_bytes.zip(record.image_bytes).map(|(a, b)| a + b);
        s.known_cost_usd += record.billed_cost.unwrap_or(0.0);
        s.cost_usd = s.cost_usd.zip(record.billed_cost).map(|(a, b)| a + b);
    }
    if s.attempts == 0 {
        s.usage_complete = false;
        s.cost_usd = None;
        s.request_bytes = None;
        s.image_bytes = None;
    }
    s.strict_completion_rate = (s.attempts > 0).then(|| s.passes as f64 / s.attempts as f64);
    s.verified_latency_ms_p50 = percentile(&verified, 0.50);
    s.verified_latency_ms_p95 = percentile(&verified, 0.95);
    s.all_latency_ms_p50 = percentile(&all, 0.50);
    s.cost_per_success = cost_per_success(s.cost_usd, s.passes);
    s.cost_per_attempt = s
        .cost_usd
        .filter(|_| s.attempts > 0)
        .map(|cost| cost / s.attempts as f64);
    s
}

/// Total cost of every attempt over verified successes. `None` when the
/// cost is unknown or nothing succeeded: undefined, not zero.
pub fn cost_per_success(total_cost: Option<f64>, successes: u64) -> Option<f64> {
    total_cost
        .filter(|_| successes > 0)
        .map(|cost| cost / successes as f64)
}

/// A small, seedable generator (SplitMix64), so a bootstrap is
/// reproducible from its seed without another dependency.
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() as u128 * n as u128) >> 64) as usize
    }
}

/// The mean of `diffs` and a 95% percentile-bootstrap interval for it,
/// resampling the values (one per task) with replacement.
pub fn bootstrap_mean_ci(diffs: &[f64], resamples: usize, seed: u64) -> Option<(f64, f64, f64)> {
    if diffs.is_empty() {
        return None;
    }
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let mut rng = SplitMix64::new(seed);
    let means: Vec<f64> = (0..resamples.max(1))
        .map(|_| {
            (0..diffs.len())
                .map(|_| diffs[rng.below(diffs.len())])
                .sum::<f64>()
                / diffs.len() as f64
        })
        .collect();
    Some((mean, percentile(&means, 0.025)?, percentile(&means, 0.975)?))
}

/// A metric compared between two arms over the tasks both ran.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paired {
    pub metric: String,
    pub baseline: String,
    pub arm: String,
    /// Tasks with a value in both arms: the unit of resampling.
    pub tasks: usize,
    /// Mean over tasks of (arm − baseline), each side averaged over runs.
    pub mean_diff: Option<f64>,
    pub ci95_low: Option<f64>,
    pub ci95_high: Option<f64>,
    pub per_task: BTreeMap<String, f64>,
}

/// The metrics compared, and how each is read from a record. A record
/// without a value (unknown usage, unknown cost) is left out of that
/// metric's per-task mean.
pub const PAIRED_METRICS: &[&str] = &[
    "strict_success",
    "total_ms",
    "uncached_input",
    "cached_input",
    "cache_write",
    "output",
    "model_calls",
    "request_bytes",
    "image_bytes",
    "billed_cost",
    "false_success",
];

fn metric(record: &RunRecord, name: &str) -> Option<f64> {
    match name {
        "strict_success" => Some(if record.outcome == Outcome::Pass {
            1.0
        } else {
            0.0
        }),
        "total_ms" => Some(record.total_ms),
        "uncached_input" => record.uncached_input.map(|v| v as f64),
        "cached_input" => record.cached_input.map(|v| v as f64),
        "cache_write" => record.cache_write.map(|v| v as f64),
        "output" => record.output.map(|v| v as f64),
        "request_bytes" => record.request_bytes.map(|v| v as f64),
        "image_bytes" => record.image_bytes.map(|v| v as f64),
        "false_success" => Some(if record.outcome == Outcome::FalseSuccess {
            1.0
        } else {
            0.0
        }),
        "model_calls" => record.model_calls.map(|v| v as f64),
        "billed_cost" => record.billed_cost,
        _ => None,
    }
}

/// Paired per-task differences of `arm` against `baseline`, with a
/// bootstrap 95% interval resampling tasks (not runs: repeated runs of
/// one task are not independent tasks).
pub fn compare(
    records: &[RunRecord],
    baseline: &str,
    arm: &str,
    resamples: usize,
    seed: u64,
) -> Vec<Paired> {
    let per_task = |name: &str, metric_name: &str| {
        let mut sums: BTreeMap<String, (f64, u32)> = BTreeMap::new();
        for record in records
            .iter()
            .filter(|r| r.arm == name && r.outcome != Outcome::Unsupported)
        {
            if let Some(value) = metric(record, metric_name) {
                let entry = sums.entry(record.task_id.clone()).or_default();
                entry.0 += value;
                entry.1 += 1;
            }
        }
        sums.into_iter()
            .map(|(task, (sum, n))| (task, sum / f64::from(n)))
            .collect::<BTreeMap<_, _>>()
    };
    PAIRED_METRICS
        .iter()
        .map(|name| {
            let (base, other) = (per_task(baseline, name), per_task(arm, name));
            let diffs: BTreeMap<String, f64> = other
                .iter()
                .filter_map(|(task, value)| Some((task.clone(), value - base.get(task)?)))
                .collect();
            let values: Vec<f64> = diffs.values().copied().collect();
            let ci = bootstrap_mean_ci(&values, resamples, seed);
            Paired {
                metric: (*name).to_string(),
                baseline: baseline.to_string(),
                arm: arm.to_string(),
                tasks: values.len(),
                mean_diff: ci.map(|c| c.0),
                ci95_low: ci.map(|c| c.1),
                ci95_high: ci.map(|c| c.2),
                per_task: diffs,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        arm: &str,
        task: &str,
        outcome: Outcome,
        total_ms: f64,
        cost: Option<f64>,
    ) -> RunRecord {
        RunRecord {
            arm: arm.into(),
            task_id: task.into(),
            outcome,
            total_ms,
            billed_cost: cost,
            uncached_input: Some(100),
            output: Some(10),
            usage_complete: true,
            ..RunRecord::default()
        }
    }

    #[test]
    fn outcomes_distinguish_false_success_from_honest_failure() {
        let ok = Observed {
            setup_ok: true,
            agent_started: true,
            agent_exited_ok: true,
            ..Observed::default()
        };
        assert_eq!(
            classify(Observed {
                check_passed: true,
                ..ok
            }),
            (Outcome::Pass, None)
        );
        assert_eq!(
            classify(Observed {
                claimed_success: true,
                ..ok
            }),
            (Outcome::FalseSuccess, Some("check_failed"))
        );
        assert_eq!(classify(ok), (Outcome::Fail, Some("check_failed")));
        assert_eq!(
            classify(Observed {
                check_passed: true,
                forbidden_effects: 1,
                claimed_success: true,
                ..ok
            }),
            (Outcome::FalseSuccess, Some("forbidden_effect"))
        );
        assert_eq!(
            classify(Observed {
                timed_out: true,
                check_passed: true,
                ..ok
            }),
            (Outcome::Timeout, Some("timeout")),
            "a late success is not a success"
        );
        assert_eq!(
            classify(Observed {
                agent_exited_ok: false,
                ..ok
            }),
            (Outcome::Error, Some("agent_error"))
        );
        assert_eq!(
            classify(Observed {
                setup_ok: false,
                ..ok
            }),
            (Outcome::Error, Some("setup_failed"))
        );
    }

    #[test]
    fn a_claim_needs_a_normal_end_and_no_reported_blocker() {
        assert!(claims_success(
            true,
            true,
            "Renamed the function; tests pass."
        ));
        assert!(claims_success(true, true, ""));
        assert!(!claims_success(true, true, "I could not run the tests."));
        assert!(!claims_success(true, true, "Tôi không thể sửa lỗi này."));
        assert!(!claims_success(true, false, "Done."));
        assert!(!claims_success(false, true, "Done."));
        // The outcome is in the lead; a difficulty overcome later is not a
        // blocker.
        assert!(claims_success(
            true,
            true,
            "I added the line and saved the file in place.\n\nWhat happened: my first try \
             could not open the dialog, so I used another way."
        ));
        assert!(!claims_success(
            true,
            true,
            "I did not add the line. The dialog never opened.\n\nDetails follow."
        ));
        assert!(!claims_success(
            true,
            true,
            "The document is saved, but I could not check the file on disk."
        ));
        // Two answers the pilot ablation first counted as false successes.
        assert!(!claims_success(
            true,
            true,
            "The export was not downloaded. The browser refused the submit click, so I stopped \
             there."
        ));
        assert!(!claims_success(
            true,
            true,
            "I haven't downloaded the export. The browser refused the Export CSV click."
        ));
    }

    #[test]
    fn summaries_count_every_attempt_and_leave_undefined_costs_undefined() {
        let records = [
            record("a", "t1", Outcome::Pass, 1000.0, Some(0.02)),
            record("a", "t2", Outcome::Timeout, 9000.0, Some(0.05)),
            record("a", "t3", Outcome::FalseSuccess, 2000.0, Some(0.01)),
            record("a", "t4", Outcome::Pass, 3000.0, Some(0.02)),
            record("a", "b1", Outcome::Unsupported, 0.0, None),
        ];
        let refs: Vec<&RunRecord> = records.iter().collect();
        let s = summarize("a", &refs);
        assert_eq!((s.attempts, s.passes, s.unsupported), (4, 2, 1));
        assert_eq!(s.strict_completion_rate, Some(0.5));
        assert_eq!((s.false_successes, s.timeouts), (1, 1));
        assert_eq!(s.verified_latency_ms_p50, Some(2000.0));
        assert_eq!(s.verified_latency_ms_p95, Some(2900.0));
        let cost = s.cost_usd.unwrap();
        assert!((cost - 0.10).abs() < 1e-12);
        assert!(
            (s.cost_per_success.unwrap() - 0.05).abs() < 1e-12,
            "all attempts' cost / successes"
        );
        assert_eq!(s.uncached_input, 400);

        let failed = [record("a", "t1", Outcome::Fail, 1.0, Some(0.3))];
        let s = summarize("a", &failed.iter().collect::<Vec<_>>());
        assert_eq!(s.cost_per_success, None, "no successes: undefined");
        assert_eq!(s.verified_latency_ms_p50, None);
        assert_eq!(cost_per_success(Some(1.0), 0), None);
        assert_eq!(cost_per_success(None, 3), None);

        let unknown = [
            record("a", "t1", Outcome::Pass, 1.0, Some(0.3)),
            record("a", "t2", Outcome::Pass, 1.0, None),
        ];
        let s = summarize("a", &unknown.iter().collect::<Vec<_>>());
        assert_eq!(s.cost_usd, None);
        assert_eq!(s.cost_per_success, None);
        assert!((s.known_cost_usd - 0.3).abs() < 1e-12);
    }

    #[test]
    fn percentiles_interpolate() {
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[5.0], 0.95), Some(5.0));
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.5), Some(2.5));
        assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 1.0), Some(4.0));
    }

    #[test]
    fn the_bootstrap_is_reproducible_from_its_seed_and_brackets_the_mean() {
        let diffs = [0.0, 1.0, 1.0, 0.0, 1.0, -1.0, 0.0, 1.0];
        let a = bootstrap_mean_ci(&diffs, 2000, 7).unwrap();
        let b = bootstrap_mean_ci(&diffs, 2000, 7).unwrap();
        assert_eq!(a, b);
        assert!((a.0 - 0.375).abs() < 1e-12);
        assert!(a.1 < a.0 && a.0 < a.2, "{a:?}");
        assert!(a.1 >= -1.0 && a.2 <= 1.0);
        let spread = [0.13, 0.71, 1.37, -0.29, 0.05, 2.11, -1.4, 0.88];
        assert_ne!(
            bootstrap_mean_ci(&spread, 200, 7),
            bootstrap_mean_ci(&spread, 200, 8),
            "another seed, another resample"
        );
        assert_eq!(bootstrap_mean_ci(&[], 100, 1), None);
        // Identical values: the interval collapses on them.
        assert_eq!(
            bootstrap_mean_ci(&[2.0, 2.0, 2.0], 50, 1),
            Some((2.0, 2.0, 2.0))
        );
    }

    #[test]
    fn arms_are_compared_task_by_task() {
        let records = vec![
            record("base", "t1", Outcome::Pass, 1000.0, Some(0.02)),
            record("base", "t1", Outcome::Fail, 3000.0, Some(0.02)),
            record("base", "t2", Outcome::Fail, 2000.0, None),
            record("lean", "t1", Outcome::Pass, 500.0, Some(0.01)),
            record("lean", "t1", Outcome::Pass, 500.0, Some(0.01)),
            record("lean", "t2", Outcome::Pass, 1000.0, Some(0.01)),
            record("lean", "t3", Outcome::Pass, 1000.0, Some(0.01)),
        ];
        let paired = compare(&records, "base", "lean", 500, 1);
        let get = |name: &str| paired.iter().find(|p| p.metric == name).unwrap();
        let success = get("strict_success");
        assert_eq!(success.tasks, 2, "t3 ran only in one arm");
        assert_eq!(success.per_task["t1"], 0.5);
        assert_eq!(success.per_task["t2"], 1.0);
        assert_eq!(success.mean_diff, Some(0.75));
        let time = get("total_ms");
        assert_eq!(time.per_task["t1"], -1500.0);
        let cost = get("billed_cost");
        assert_eq!(cost.tasks, 1, "t2's baseline cost is unknown");
        assert_eq!(compare(&records, "base", "lean", 500, 1), paired, "seeded");
    }
}
