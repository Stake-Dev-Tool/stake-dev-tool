//! Stake-Engine-style compliance analysis (2★ / 3★) for a revision.
//!
//! This module turns the *already-parsed* per-mode lookup rows (see
//! [`crate::stats::Weighted`]) into the rich [`RevisionAnalysis`] wire object:
//! a per-mode deep-dive ([`ModeAnalysis`]) plus a global constraint table
//! ([`ConstraintRow`]) checked at two reference bet levels. Nothing here reads
//! files or the database — [`crate::stats::compute`] parses each weights CSV
//! exactly once and hands the rows to both the basic `ModeStats` path and to
//! [`mode_analysis`], so the analysis rides along the same compute pass.
//!
//! ## Definitions (kept verbatim from the M8 spec)
//! For a mode with rows `(w_i, payout_i)`: `W = Σ w`, `p_i = w_i / W`, the
//! decimal BET multiplier `m_i = payout_i / 100`, and the cost-normalized
//! `x_i = m_i / cost`. RTP `= Σ p·x`; `σ = sqrt(Σ p·x² − RTP²)`. All the
//! probability fields are integer-weight sums divided by `W` (exact), while the
//! RTP-share fields (CVaR, the ETLs, bucket contributions) accumulate `p·x` in
//! f64. Payout comparisons run in integer "hundredths" space (`m ⋛ cost`
//! becomes `payout ⋛ cost·100`) so boundaries are exact.

use std::collections::HashSet;

use protocol::{ComplianceCheck, ConstraintRow, DistBucket, ModeAnalysis, RevisionAnalysis};

use crate::stats::Weighted;

/// Documented stand-ins for Stake's per-game bet-level templates: bet-scaled
/// limits (`max_exposure`, `max_bet_cost`) are evaluated at these reference bets.
const REF_BET_2: u64 = 200;
const REF_BET_3: u64 = 1000;

/// The CVaR / streak tail mass: the worst (or rarest) 0.1% of outcomes.
const TAIL: f64 = 0.001;

/// Upper edges of the win-multiplier distribution buckets. Bucket `i` spans
/// `(EDGES[i], EDGES[i+1]]`; the final bucket `(EDGES[last], ∞)` has `to = None`.
/// The zero (losing) payout is excluded from every bucket.
const DIST_EDGES: [f64; 15] = [
    0.0, 0.1, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
];

/// Compute one mode's full deep-dive from its parsed lookup rows.
///
/// `rows` must be non-empty with a non-zero total weight (the basic
/// `ModeStats` path validates this first, so by the time we get here it holds);
/// the few divisions that would otherwise blow up are guarded anyway.
pub(crate) fn mode_analysis(mode: &str, cost: u64, rows: &[Weighted]) -> ModeAnalysis {
    let cost_u = cost.max(1);
    let cost_f = cost_u as f64;
    // `cost` expressed in payout-hundredths, so `m ⋛ cost ⟺ payout ⋛ cost_h`.
    let cost_h = cost_u.saturating_mul(100);

    let total_weight: u128 = rows.iter().map(|r| u128::from(r.weight)).sum();
    let total = total_weight as f64;

    // Single pass over the rows.
    let mut rtp = 0.0f64; // Σ p·x
    let mut sum_px2 = 0.0f64; // Σ p·x²
    let mut etl_40 = 0.0f64; // Σ_{m > 40·cost} p·x
    let mut etl_10000 = 0.0f64; // Σ_{m > 10000} p·x

    let mut zero_w: u128 = 0; // m == 0
    let mut sub_bet_w: u128 = 0; // 0 < m < cost
    let mut win_w: u128 = 0; // m >= cost
    let mut profit_w: u128 = 0; // m > cost
    let mut below_cost_w: u128 = 0; // m < cost
    let mut le_cost_w: u128 = 0; // m <= cost
    let mut tail5000_w: u128 = 0; // m >= 5000
    let mut tail10000_w: u128 = 0; // m >= 10000

    let mut max_payout: u32 = 0;
    let mut max_payout_w: u128 = 0; // total weight sitting on the top multiplier
    let mut unique: HashSet<u32> = HashSet::new();

    let etl40_threshold = cost_h.saturating_mul(40); // 40·cost in hundredths

    for r in rows {
        let wq = u128::from(r.weight);
        let p = r.weight as f64 / total;
        let x = f64::from(r.payout) / 100.0 / cost_f;
        rtp += p * x;
        sum_px2 += p * x * x;

        let pay = u64::from(r.payout);
        if pay == 0 {
            zero_w += wq;
        }
        if pay > 0 && pay < cost_h {
            sub_bet_w += wq;
        }
        if pay >= cost_h {
            win_w += wq;
        }
        if pay > cost_h {
            profit_w += wq;
        }
        if pay < cost_h {
            below_cost_w += wq;
        }
        if pay <= cost_h {
            le_cost_w += wq;
        }
        if pay >= 500_000 {
            tail5000_w += wq;
        }
        if pay >= 1_000_000 {
            tail10000_w += wq;
        }
        if pay > etl40_threshold {
            etl_40 += p * x;
        }
        if pay > 1_000_000 {
            etl_10000 += p * x;
        }

        if r.payout > max_payout {
            max_payout = r.payout;
            max_payout_w = wq;
        } else if r.payout == max_payout {
            max_payout_w += wq;
        }
        unique.insert(r.payout);
    }

    let zero_prob = zero_w as f64 / total;
    let sub_bet_prob = sub_bet_w as f64 / total;
    let win_prob = win_w as f64 / total;
    let profit_prob = profit_w as f64 / total;
    let break_even_miss_prob = below_cost_w as f64 / total;
    let le_cost_prob = le_cost_w as f64 / total;
    let tail_prob_5000 = tail5000_w as f64 / total;
    let tail_prob_10000 = tail10000_w as f64 / total;
    // hit_rate = P(m > 0), computed directly so it matches ModeStats exactly.
    let hit_rate = (total_weight - zero_w) as f64 / total;

    let max_win = f64::from(max_payout) / 100.0;
    let min_payout = rows.iter().map(|r| r.payout).min().unwrap_or(0);
    let min_win = f64::from(min_payout) / 100.0;

    let variance = (sum_px2 - rtp * rtp).max(0.0);
    let std_dev = variance.sqrt();
    let volatility = volatility_label(std_dev);

    let max_win_odds = if max_payout_w > 0 {
        total / max_payout_w as f64
    } else {
        0.0
    };

    let avg_spins_any_win = if zero_prob < 1.0 {
        Some(1.0 / (1.0 - zero_prob))
    } else {
        None
    };
    let worst_zero_streak = streak(zero_prob);
    let avg_spins_profit = if profit_prob > 0.0 {
        Some(1.0 / profit_prob)
    } else {
        None
    };
    let worst_loss_streak = streak(le_cost_prob);

    let cvar = cvar_top_tail(rows, total, cost_f);
    let distribution = distribution(rows, total, cost_f);

    let compliance = vec![
        rtp_range_check(rtp),
        max_win_check(max_win_odds),
        hit_rate_check(hit_rate),
    ];

    ModeAnalysis {
        mode: mode.to_string(),
        cost: cost_f,
        rtp,
        std_dev,
        volatility: volatility.to_string(),
        max_win,
        min_win,
        zero_prob,
        sub_bet_prob,
        win_prob,
        break_even_miss_prob,
        hit_rate,
        unique_payouts: unique.len() as u64,
        entries: rows.len() as u64,
        max_win_odds,
        avg_spins_any_win,
        worst_zero_streak,
        avg_spins_profit,
        worst_loss_streak,
        tail_prob_5000,
        tail_prob_10000,
        cvar,
        etl_40,
        etl_10000,
        etl_sum: etl_40 + etl_10000,
        distribution,
        compliance,
    }
}

/// Assemble the whole-revision analysis from the per-mode deep-dives. The base
/// (cheapest) mode carries the extra `base_cost` compliance check and supplies
/// the volatility / tail / CVaR / ETL figures of the global constraint table.
pub(crate) fn revision_analysis(mut modes: Vec<ModeAnalysis>) -> RevisionAnalysis {
    // Base mode = the cost==1 mode if present, else the cheapest. Because cost
    // is guaranteed >= 1, a cost==1 mode is always the global minimum, so
    // "first minimum cost" captures both cases. `min_by` keeps the first on ties.
    let base_idx = modes
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            a.cost
                .partial_cmp(&b.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map_or(0, |(i, _)| i);

    // Only the cheapest mode gets the base-cost check.
    let base_cost = modes[base_idx].cost;
    modes[base_idx].compliance.push(base_cost_check(base_cost));

    let max_win_over_modes = modes.iter().map(|m| m.max_win).fold(0.0f64, f64::max);
    let max_cost = modes.iter().map(|m| m.cost).fold(0.0f64, f64::max);
    let base = &modes[base_idx];

    let constraints = vec![
        bet_scaled_row(
            "max_exposure",
            "Max exposure",
            max_win_over_modes,
            15_000_000.0,
            50_000_000.0,
        ),
        single_row(
            "max_payout_multiplier",
            "Max payout multiplier",
            max_win_over_modes,
            25_000.0,
            100_000.0,
        ),
        bet_scaled_row(
            "max_bet_cost",
            "Max bet cost",
            max_cost,
            100_000.0,
            500_000.0,
        ),
        single_row(
            "cost_multiplier",
            "Cost multiplier",
            max_cost,
            1_000.0,
            1_500.0,
        ),
        range_row(
            "base_volatility",
            "Base volatility",
            base.std_dev,
            0.6,
            50.0,
            0.6,
            60.0,
        ),
        single_row(
            "tail_prob_5000",
            "Tail probability \u{2265} 5,000\u{d7}",
            base.tail_prob_5000,
            0.010,
            0.010,
        ),
        single_row(
            "tail_prob_10000",
            "Tail probability \u{2265} 10,000\u{d7}",
            base.tail_prob_10000,
            0.002,
            0.005,
        ),
        single_row("cvar", "CVaR (worst 0.1% tail)", base.cvar, 700.0, 800.0),
        single_row(
            "etl_40",
            "Expected tail loss (> 40\u{d7})",
            base.etl_40,
            0.800,
            0.900,
        ),
        single_row(
            "etl_10000",
            "Expected tail loss (> 10,000\u{d7})",
            base.etl_10000,
            0.600,
            0.800,
        ),
        single_row(
            "etl_sum",
            "Expected tail loss (combined)",
            base.etl_sum,
            1.300,
            1.500,
        ),
    ];

    let two_star_compliant = constraints.iter().all(|c| c.pass2);
    let three_star_compliant = constraints.iter().all(|c| c.pass3);
    let stars = if three_star_compliant {
        3
    } else if two_star_compliant {
        2
    } else {
        0
    };

    let max_rtp = modes.iter().map(|m| m.rtp).fold(0.0f64, f64::max);
    let min_rtp = modes.iter().map(|m| m.rtp).fold(f64::MAX, f64::min);
    let cross_mode_rtp_variance = max_rtp - min_rtp;
    let cross_mode_rtp_pass = cross_mode_rtp_variance <= 0.01;

    RevisionAnalysis {
        two_star_compliant,
        three_star_compliant,
        stars,
        cross_mode_rtp_variance,
        cross_mode_rtp_pass,
        reference_max_bet_2: REF_BET_2,
        reference_max_bet_3: REF_BET_3,
        constraints,
        modes,
    }
}

/// σ → heuristic volatility label: `< 8` low, `8..=25` medium, `> 25` high.
fn volatility_label(std_dev: f64) -> &'static str {
    if std_dev < 8.0 {
        "low"
    } else if std_dev <= 25.0 {
        "medium"
    } else {
        "high"
    }
}

/// Closed-form "1-in-1000" streak: `ceil(ln 0.001 / ln q)`. `None` in the
/// degenerate cases (`q == 0` → the event never happens; `q >= 1` → it always
/// does, so the streak is unbounded).
fn streak(q: f64) -> Option<u64> {
    if q > 0.0 && q < 1.0 {
        Some((TAIL.ln() / q.ln()).ceil() as u64)
    } else {
        None
    }
}

/// CVaR over the top 0.1% of outcomes: sort by `x` descending, take exactly
/// `0.001` probability mass (splitting the boundary row proportionally), and
/// return `(Σ slice p·x) / 0.001` — the mean `x` conditional on the best tail.
fn cvar_top_tail(rows: &[Weighted], total: f64, cost_f: f64) -> f64 {
    // Sorting by payout desc is identical to sorting by x desc (x is monotonic
    // in payout for a fixed cost).
    let mut sorted: Vec<&Weighted> = rows.iter().collect();
    sorted.sort_by_key(|r| std::cmp::Reverse(r.payout));

    let mut remaining = TAIL;
    let mut acc = 0.0f64;
    for r in sorted {
        if remaining <= 0.0 {
            break;
        }
        let p = r.weight as f64 / total;
        let x = f64::from(r.payout) / 100.0 / cost_f;
        let take = p.min(remaining);
        acc += take * x;
        remaining -= take;
    }
    acc / TAIL
}

/// Build the win-multiplier distribution, excluding `m == 0` and omitting the
/// trailing empty buckets past the biggest win.
fn distribution(rows: &[Weighted], total: f64, cost_f: f64) -> Vec<DistBucket> {
    struct Acc {
        weight: u128,
        px: f64,
        payouts: HashSet<u32>,
    }
    let mut buckets: Vec<Acc> = (0..DIST_EDGES.len())
        .map(|_| Acc {
            weight: 0,
            px: 0.0,
            payouts: HashSet::new(),
        })
        .collect();

    for r in rows {
        if r.payout == 0 {
            continue; // the zero payout is `zero_prob`, never a bucket
        }
        let m = f64::from(r.payout) / 100.0;
        let idx = bucket_index(m);
        let acc = &mut buckets[idx];
        acc.weight += u128::from(r.weight);
        acc.px += (r.weight as f64 / total) * (m / cost_f);
        acc.payouts.insert(r.payout);
    }

    // Omit trailing empty buckets past the max win (= last non-empty bucket).
    let keep = buckets
        .iter()
        .rposition(|b| b.weight > 0)
        .map_or(0, |i| i + 1);

    buckets
        .iter()
        .take(keep)
        .enumerate()
        .map(|(i, b)| {
            let probability = b.weight as f64 / total;
            DistBucket {
                from: DIST_EDGES[i],
                to: DIST_EDGES.get(i + 1).copied(),
                count: b.payouts.len() as u64,
                probability,
                effective_hit_rate: if probability > 0.0 {
                    Some(1.0 / probability)
                } else {
                    None
                },
                rtp_contribution: b.px,
            }
        })
        .collect()
}

/// Bucket a strictly-positive multiplier `m` under the `(from, to]` rule: the
/// first `i` whose upper edge `>= m`, else the open final bucket.
fn bucket_index(m: f64) -> usize {
    for i in 0..DIST_EDGES.len() - 1 {
        if m <= DIST_EDGES[i + 1] {
            return i;
        }
    }
    DIST_EDGES.len() - 1
}

// --- constraint-row builders ------------------------------------------------

/// A single-value metric compared against an upper limit at each star.
fn single_row(key: &str, label: &str, value: f64, limit2: f64, limit3: f64) -> ConstraintRow {
    ConstraintRow {
        key: key.to_string(),
        label: label.to_string(),
        value: Some(value),
        value2: None,
        value3: None,
        limit2_low: None,
        limit2,
        limit3_low: None,
        limit3,
        pass2: value <= limit2,
        pass3: value <= limit3,
    }
}

/// A bet-scaled metric: `per_bet` is multiplied by each reference bet to fill
/// `value2` / `value3`, then compared against the star's upper limit.
fn bet_scaled_row(key: &str, label: &str, per_bet: f64, limit2: f64, limit3: f64) -> ConstraintRow {
    let value2 = per_bet * REF_BET_2 as f64;
    let value3 = per_bet * REF_BET_3 as f64;
    ConstraintRow {
        key: key.to_string(),
        label: label.to_string(),
        value: None,
        value2: Some(value2),
        value3: Some(value3),
        limit2_low: None,
        limit2,
        limit3_low: None,
        limit3,
        pass2: value2 <= limit2,
        pass3: value3 <= limit3,
    }
}

/// A range metric that must sit inside `[low, high]` at each star.
fn range_row(
    key: &str,
    label: &str,
    value: f64,
    low2: f64,
    high2: f64,
    low3: f64,
    high3: f64,
) -> ConstraintRow {
    ConstraintRow {
        key: key.to_string(),
        label: label.to_string(),
        value: Some(value),
        value2: None,
        value3: None,
        limit2_low: Some(low2),
        limit2: high2,
        limit3_low: Some(low3),
        limit3: high3,
        pass2: value >= low2 && value <= high2,
        pass3: value >= low3 && value <= high3,
    }
}

// --- per-mode compliance checks ---------------------------------------------

fn rtp_range_check(rtp: f64) -> ComplianceCheck {
    let low = 0.90;
    let high = 0.967;
    ComplianceCheck {
        check: "rtp_range".to_string(),
        label: "RTP within range".to_string(),
        expected: format!("{:.1}% \u{2013} {:.2}%", low * 100.0, high * 100.0),
        result: format!("{:.2}%", rtp * 100.0),
        pass: rtp >= low && rtp <= high,
    }
}

fn max_win_check(max_win_odds: f64) -> ComplianceCheck {
    let limit = 20_000_000.0;
    ComplianceCheck {
        check: "max_win_achievability".to_string(),
        label: "Max win achievable".to_string(),
        expected: format!("Odds \u{2264} 1 in {:.2}M", limit / 1e6),
        result: format!("1 in {:.2}M", max_win_odds / 1e6),
        pass: max_win_odds <= limit,
    }
}

fn hit_rate_check(hit_rate: f64) -> ComplianceCheck {
    let limit = 0.05;
    ComplianceCheck {
        check: "hit_rate".to_string(),
        label: "Minimum hit rate".to_string(),
        expected: format!("\u{2264} 1 in {:.0}", 1.0 / limit),
        result: if hit_rate > 0.0 {
            format!("1 in {:.1}", 1.0 / hit_rate)
        } else {
            "never".to_string()
        },
        pass: hit_rate >= limit,
    }
}

fn base_cost_check(cost: f64) -> ComplianceCheck {
    ComplianceCheck {
        check: "base_cost".to_string(),
        label: "Base bet cost".to_string(),
        expected: "Cost multiplier = 1.0x".to_string(),
        result: format!("{cost:.2}"),
        pass: cost == 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOL: f64 = 1e-9;

    fn rows(pairs: &[(u64, u32)]) -> Vec<Weighted> {
        pairs
            .iter()
            .map(|&(weight, payout)| Weighted { weight, payout })
            .collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < TOL
    }

    fn constraint<'a>(a: &'a RevisionAnalysis, key: &str) -> &'a ConstraintRow {
        a.constraints
            .iter()
            .find(|c| c.key == key)
            .unwrap_or_else(|| panic!("missing constraint {key}"))
    }

    fn check<'a>(m: &'a ModeAnalysis, name: &str) -> &'a ComplianceCheck {
        m.compliance
            .iter()
            .find(|c| c.check == name)
            .unwrap_or_else(|| panic!("missing check {name}"))
    }

    /// The M8 micro-fixture — every field derived by hand in the comments below.
    ///
    /// rows (cost 1): 0:9000/0, 1:900/100, 2:90/5000, 3:10/42000; W = 10000.
    /// p = [0.9, 0.09, 0.009, 0.001]; m = x = [0, 1, 50, 420].
    #[test]
    fn micro_fixture_mode_analysis_matches_hand_values() {
        let r = rows(&[(9000, 0), (900, 100), (90, 5000), (10, 42000)]);
        let m = mode_analysis("base", 1, &r);

        assert_eq!(m.mode, "base");
        assert!(close(m.cost, 1.0));
        // RTP = 0*.9 + 1*.09 + 50*.009 + 420*.001 = 0.96
        assert!(close(m.rtp, 0.96), "rtp {}", m.rtp);
        // Σp·x² = .09 + .009*2500 + .001*176400 = 198.99; Var = 198.99 - .9216 = 198.0684
        assert!(
            close(m.std_dev, 198.0684_f64.sqrt()),
            "std_dev {}",
            m.std_dev
        );
        assert_eq!(m.volatility, "medium");
        assert!(close(m.max_win, 420.0));
        assert!(close(m.min_win, 0.0));
        assert!(close(m.zero_prob, 0.9));
        assert!(close(m.sub_bet_prob, 0.0));
        assert!(close(m.win_prob, 0.1));
        assert!(close(m.break_even_miss_prob, 0.9));
        assert!(close(m.hit_rate, 0.1));
        assert_eq!(m.unique_payouts, 4);
        assert_eq!(m.entries, 4);
        // 1 / P(m == 420) = 1 / 0.001 = 1000
        assert!(close(m.max_win_odds, 1000.0), "odds {}", m.max_win_odds);
        // 1 / (1 - 0.9) = 10
        assert!(close(m.avg_spins_any_win.unwrap(), 10.0));
        // ceil(ln .001 / ln .9) = ceil(65.563) = 66
        assert_eq!(m.worst_zero_streak, Some(66));
        // 1 / P(m > 1) = 1 / 0.01 = 100
        assert!(close(m.avg_spins_profit.unwrap(), 100.0));
        // ceil(ln .001 / ln P(m<=1)=.99) = ceil(687.216) = 688
        assert_eq!(m.worst_loss_streak, Some(688));
        assert!(close(m.tail_prob_5000, 0.0));
        assert!(close(m.tail_prob_10000, 0.0));
        // Top 0.1% is exactly the m=420 row (p=0.001): cvar = 0.001*420/0.001 = 420
        assert!(close(m.cvar, 420.0), "cvar {}", m.cvar);
        // Σ_{m>40} p·x = 50*.009 + 420*.001 = 0.45 + 0.42 = 0.87
        assert!(close(m.etl_40, 0.87), "etl_40 {}", m.etl_40);
        assert!(close(m.etl_10000, 0.0));
        assert!(close(m.etl_sum, 0.87));

        // Distribution: non-empty at (0.1,1], (20,50], (200,500]; trailing empty
        // buckets past 420 are omitted, so 10 buckets remain (indices 0..=9).
        assert_eq!(m.distribution.len(), 10);
        let b1 = &m.distribution[1];
        assert!(close(b1.from, 0.1) && b1.to == Some(1.0));
        assert_eq!(b1.count, 1);
        assert!(close(b1.probability, 0.09));
        assert!(close(b1.effective_hit_rate.unwrap(), 1.0 / 0.09));
        assert!(close(b1.rtp_contribution, 0.09));
        let b6 = &m.distribution[6];
        assert!(close(b6.from, 20.0) && b6.to == Some(50.0));
        assert_eq!(b6.count, 1);
        assert!(close(b6.probability, 0.009));
        assert!(close(b6.rtp_contribution, 0.45));
        let b9 = &m.distribution[9];
        assert!(close(b9.from, 200.0) && b9.to == Some(500.0));
        assert_eq!(b9.count, 1);
        assert!(close(b9.probability, 0.001));
        assert!(close(b9.rtp_contribution, 0.42));
        // An interior empty bucket is kept (it precedes the max win).
        assert_eq!(m.distribution[0].count, 0);
        assert!(close(m.distribution[0].probability, 0.0));
        assert!(m.distribution[0].effective_hit_rate.is_none());

        // mode_analysis emits the 3 universal checks; base_cost is added at the
        // revision level to the cheapest mode only.
        assert_eq!(m.compliance.len(), 3);
        let rtp = check(&m, "rtp_range");
        assert_eq!(rtp.expected, "90.0% \u{2013} 96.70%");
        assert_eq!(rtp.result, "96.00%");
        assert!(rtp.pass);
        assert_eq!(check(&m, "hit_rate").result, "1 in 10.0");
        assert_eq!(check(&m, "hit_rate").expected, "\u{2264} 1 in 20");
    }

    /// The whole-revision view of the micro-fixture: a single cost-1 mode. The
    /// interesting bit is etl_40 = 0.87, which fails the 2★ cap (0.80) but clears
    /// the 3★ cap (0.90) — so the game is 3★ *without* being 2★.
    #[test]
    fn micro_fixture_revision_is_three_star_not_two() {
        let r = rows(&[(9000, 0), (900, 100), (90, 5000), (10, 42000)]);
        let a = revision_analysis(vec![mode_analysis("base", 1, &r)]);

        assert!(!a.two_star_compliant);
        assert!(a.three_star_compliant);
        assert_eq!(a.stars, 3);
        assert!(close(a.cross_mode_rtp_variance, 0.0));
        assert!(a.cross_mode_rtp_pass);
        assert_eq!(a.reference_max_bet_2, 200);
        assert_eq!(a.reference_max_bet_3, 1000);
        assert_eq!(a.constraints.len(), 11);

        let exposure = constraint(&a, "max_exposure");
        assert!(exposure.value.is_none());
        assert!(close(exposure.value2.unwrap(), 84_000.0)); // 420 * 200
        assert!(close(exposure.value3.unwrap(), 420_000.0)); // 420 * 1000
        assert!(exposure.pass2 && exposure.pass3);

        let cost_mult = constraint(&a, "cost_multiplier");
        assert!(close(cost_mult.value.unwrap(), 1.0));
        assert!(cost_mult.pass2 && cost_mult.pass3);

        let etl40 = constraint(&a, "etl_40");
        assert!(close(etl40.value.unwrap(), 0.87));
        assert!(!etl40.pass2, "etl_40 should fail the 2-star cap");
        assert!(etl40.pass3, "etl_40 should clear the 3-star cap");

        let vol = constraint(&a, "base_volatility");
        assert_eq!(vol.limit2_low, Some(0.6));
        assert!(close(vol.limit2, 50.0));
        assert!(vol.pass2 && vol.pass3);

        // The cheapest mode gains the base_cost check (4 total), and it passes.
        assert_eq!(a.modes[0].compliance.len(), 4);
        let base_cost = check(&a.modes[0], "base_cost");
        assert_eq!(base_cost.result, "1.00");
        assert!(base_cost.pass);
    }

    /// A cost-100 mode: proves RTP / CVaR / ETL / bucket contributions use the
    /// cost-normalized x = m/cost while max_win reports the raw multiplier m.
    ///
    /// rows: 0:8000/0, 1:1900/10000 (m=100, x=1), 2:100/200000 (m=2000, x=20).
    #[test]
    fn cost_100_mode_uses_x_not_m() {
        let r = rows(&[(8000, 0), (1900, 10000), (100, 200000)]);
        let m = mode_analysis("buy", 100, &r);

        assert!(close(m.cost, 100.0));
        // RTP = 0.19*1 + 0.01*20 = 0.39 (x), NOT 0.19*100 + 0.01*2000 = 39 (m).
        assert!(close(m.rtp, 0.39), "rtp {}", m.rtp);
        assert!(close(m.max_win, 2000.0)); // raw multiplier
        assert!(close(m.win_prob, 0.2)); // m >= 100
        assert!(close(m.sub_bet_prob, 0.0)); // nothing in (0, 100)
        // Top 0.1% is a slice of the m=2000 (x=20) row: cvar = 20.
        assert!(close(m.cvar, 20.0), "cvar {}", m.cvar);
        // 40*cost = 4000 > 2000, so nothing lands beyond it.
        assert!(close(m.etl_40, 0.0));

        // m=100 falls in (50,100] (index 7); its contribution is p·x = 0.19,
        // which would be 19.0 if the code wrongly used m instead of x.
        assert_eq!(m.distribution.len(), 12); // trailing empties past m=2000 dropped
        assert!(close(m.distribution[7].from, 50.0));
        assert!(close(m.distribution[7].rtp_contribution, 0.19));
        assert!(close(m.distribution[11].from, 1000.0));
        assert_eq!(m.distribution[11].to, Some(2000.0));
        assert!(close(m.distribution[11].rtp_contribution, 0.2));
    }

    /// A non-compliant revision: a base mode at RTP ~0.85 (fails rtp_range) and a
    /// cost-2000 buy mode (fails cost_multiplier and max_bet_cost at both stars).
    #[test]
    fn compliance_failures_yield_zero_stars() {
        let base = mode_analysis("base", 1, &rows(&[(1500, 0), (8500, 100)])); // rtp 0.85
        let buy = mode_analysis("buy", 2000, &rows(&[(9000, 0), (1000, 400_000)]));
        let a = revision_analysis(vec![base, buy]);

        assert!(!a.two_star_compliant);
        assert!(!a.three_star_compliant);
        assert_eq!(a.stars, 0);

        // Cost caps fail at BOTH stars.
        let cost_mult = constraint(&a, "cost_multiplier");
        assert!(close(cost_mult.value.unwrap(), 2000.0));
        assert!(!cost_mult.pass2 && !cost_mult.pass3);
        let bet_cost = constraint(&a, "max_bet_cost");
        assert!(close(bet_cost.value2.unwrap(), 400_000.0)); // 2000 * 200
        assert!(!bet_cost.pass2 && !bet_cost.pass3);

        // The base (cheapest, cost-1) mode fails the RTP band.
        let rtp = check(&a.modes[0], "rtp_range");
        assert_eq!(a.modes[0].mode, "base");
        assert_eq!(rtp.result, "85.00%");
        assert!(!rtp.pass);

        // Modes disagree on RTP by a lot.
        assert!(!a.cross_mode_rtp_pass);
    }
}
