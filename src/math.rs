//! Exact integer ports of the on-chain math in `apex::scorer`, `apex::bridge`
//! and `apex::apex_math`. Every operation replicates the Move code's
//! truncating integer arithmetic — do NOT algebraically simplify, or the
//! preview values will diverge from what the chain computes at resolve.

pub const FLOAT_SCALING: u64 = 1_000_000_000;
const FLOAT_SCALING_U128: u128 = 1_000_000_000;
const MAX_TIME_BONUS: u64 = 2_500_000_000;
const SECONDS_PER_YEAR: u64 = 31_536_000;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MathError {
    #[error("delta is smaller than the oracle tick size")]
    DeltaTooSmall,
    #[error("snapped range exceeds the oracle strike grid")]
    BoundsExceeded,
    #[error("SVI params yield negative implied variance")]
    NegativeVariance,
}

/// Mirror of deepbook_predict::i64::I64 — sign-magnitude signed value,
/// 1e9-scaled like everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct I64Fp {
    pub magnitude: u64,
    pub is_negative: bool,
}

impl I64Fp {
    pub fn new(magnitude: u64, is_negative: bool) -> Self {
        Self { magnitude, is_negative }
    }

    fn to_i128(self) -> i128 {
        if self.is_negative {
            -(self.magnitude as i128)
        } else {
            self.magnitude as i128
        }
    }

    /// (a × b) / 1e9 with truncation on the magnitude product, sign by xor —
    /// matches i64::mul_scaled.
    fn mul_scaled(self, other: I64Fp) -> I64Fp {
        let mag = ((self.magnitude as u128) * (other.magnitude as u128) / FLOAT_SCALING_U128) as u64;
        let neg = mag != 0 && (self.is_negative != other.is_negative);
        I64Fp::new(mag, neg)
    }

    /// m² / 1e9 — always non-negative, matches i64::square_scaled.
    fn square_scaled(self) -> u64 {
        ((self.magnitude as u128) * (self.magnitude as u128) / FLOAT_SCALING_U128) as u64
    }

    fn neg(self) -> I64Fp {
        I64Fp::new(self.magnitude, !self.is_negative && self.magnitude != 0)
    }
}

// === apex::scorer ===

/// Accuracy score in [0, 1e9].
/// error_bps = |v − r| × 10_000 / r; 0 if error_bps > epsilon_bps,
/// else 1e18 / (1e9 + error_bps × 1e6).
pub fn acc_score(v: u64, r: u64, epsilon_bps: u64) -> u64 {
    let diff = v.abs_diff(r);
    let error_bps = ((diff as u128) * 10_000 / (r as u128)) as u64;
    if error_bps > epsilon_bps {
        return 0;
    }
    let denom = FLOAT_SCALING_U128 + (error_bps as u128) * 1_000_000;
    (FLOAT_SCALING_U128 * FLOAT_SCALING_U128 / denom) as u64
}

/// Time bonus in [1e9, 2.5e9]. Earlier commits score higher.
#[allow(clippy::manual_clamp, clippy::implicit_saturating_sub)]
pub fn time_bonus(t_commit: u64, t_open: u64, t_cutoff: u64) -> u64 {
    if t_cutoff <= t_open {
        return FLOAT_SCALING;
    }
    let window = t_cutoff - t_open;
    let remaining = if t_commit <= t_open {
        window
    } else if t_commit >= t_cutoff {
        0
    } else {
        t_cutoff - t_commit
    };

    let t_scaled = (remaining as u128) * FLOAT_SCALING_U128 / (window as u128);
    let t_sq = t_scaled * t_scaled / FLOAT_SCALING_U128;
    let bonus = FLOAT_SCALING_U128 + 1_500_000_000u128 * t_sq / FLOAT_SCALING_U128;
    if bonus > MAX_TIME_BONUS as u128 {
        MAX_TIME_BONUS
    } else {
        bonus as u64
    }
}

/// Composite weight = entry_fee × acc × time / 1e9². Two-step truncating
/// division exactly as on chain.
pub fn composite_weight(entry_fee: u64, acc: u64, time: u64) -> u128 {
    let w1 = (entry_fee as u128) * (acc as u128) / FLOAT_SCALING_U128;
    w1 * (time as u128) / FLOAT_SCALING_U128
}

// === apex::apex_math ===

/// Fixed-point square root: input/output u64 scaled by 1e9.
pub fn sqrt_fp(x: u64) -> u64 {
    sqrt_floor((x as u128) * FLOAT_SCALING_U128) as u64
}

/// Integer floor-sqrt on u128. Newton-Raphson from above with monotone
/// convergence — exact floor, same result as the on-chain OpenZeppelin port.
fn sqrt_floor(a: u128) -> u128 {
    if a <= 1 {
        return a;
    }
    // Seed: 2^ceil(bits/2) ≥ sqrt(a), so Newton descends monotonically.
    let mut xn = 1u128 << ((128 - a.leading_zeros()).div_ceil(2));
    loop {
        let next = (xn + a / xn) >> 1;
        if next >= xn {
            break;
        }
        xn = next;
    }
    if xn > a / xn {
        xn - 1
    } else {
        xn
    }
}

// === apex::bridge ===

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RangeParams {
    pub lower_snapped: u64,
    pub upper_snapped: u64,
    pub atm_iv: u64,
    pub delta: u64,
}

/// Evaluates the SVI smile at log-moneyness k=0 to derive ATM implied vol.
pub fn svi_atm_iv(a: u64, b: u64, rho: I64Fp, m: I64Fp, sigma: u64) -> Result<u64, MathError> {
    let rho_m = rho.mul_scaled(m);
    let neg_rho_m = rho_m.neg();

    let m_sq = m.square_scaled();
    let sigma_sq = ((sigma as u128) * (sigma as u128) / FLOAT_SCALING_U128) as u64;
    let sqrt_result = sqrt_fp(m_sq + sigma_sq);

    let inner = neg_rho_m.to_i128() + sqrt_result as i128;
    if inner < 0 {
        return Err(MathError::NegativeVariance);
    }

    let b_x_inner = ((b as u128) * (inner as u128) / FLOAT_SCALING_U128) as u64;
    Ok(sqrt_fp(a + b_x_inner))
}

/// delta = ATM_IV × sqrt(T / year) × V × k / 1e9³
pub fn compute_delta(atm_iv_val: u64, t_secs: u64, v: u64, k: u64) -> u64 {
    let t_ratio = (t_secs as u128) * FLOAT_SCALING_U128 / (SECONDS_PER_YEAR as u128);
    let sqrt_t = sqrt_fp(t_ratio as u64) as u128;
    let step1 = (atm_iv_val as u128) * sqrt_t / FLOAT_SCALING_U128;
    let step2 = step1 * (v as u128);
    (step2 * (k as u128) / FLOAT_SCALING_U128 / FLOAT_SCALING_U128) as u64
}

/// Snaps [V−delta, V+delta] to the oracle's discrete strike grid:
/// lower floors, upper ceils, both to min_strike + N × tick_size slots.
pub fn snap_to_grid(
    v: u64,
    delta_val: u64,
    min_strike: u64,
    tick_size: u64,
    max_strike: u64,
) -> Result<RangeParams, MathError> {
    if delta_val < tick_size {
        return Err(MathError::DeltaTooSmall);
    }
    if v < delta_val + min_strike {
        return Err(MathError::BoundsExceeded);
    }
    let lower_raw = v - delta_val;
    let lower = (lower_raw - min_strike) / tick_size * tick_size + min_strike;

    let upper_raw = v + delta_val;
    if upper_raw > max_strike {
        return Err(MathError::BoundsExceeded);
    }
    let off_upper = upper_raw - min_strike;
    let remainder = off_upper % tick_size;
    let ceil_ticks = off_upper / tick_size + if remainder > 0 { 1 } else { 0 };
    let upper = ceil_ticks * tick_size + min_strike;
    if upper > max_strike {
        return Err(MathError::BoundsExceeded);
    }

    Ok(RangeParams {
        lower_snapped: lower,
        upper_snapped: upper,
        atm_iv: 0,
        delta: delta_val,
    })
}

// === apex::apex_pool::do_resolve per-participant scoring ===

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParticipantScore {
    pub leg_hits: u8,
    pub avg_acc: u64,
    pub time_bonus: u64,
    pub composite_weight: u128,
}

/// Mirrors the per-participant loop in `do_resolve`: average accuracy across
/// legs (truncating), true-parlay weight (0 unless every leg hits epsilon).
#[allow(dead_code)]
pub fn score_participant(
    predicted_prices: &[u64],
    oracle_results: &[u64],
    epsilon_bps: u64,
    entry_fee: u64,
    t_commit: u64,
    t_open: u64,
    t_cutoff: u64,
) -> ParticipantScore {
    let leg_count = oracle_results.len() as u64;
    let mut leg_hits = 0u8;
    let mut total_acc = 0u64;
    for (v, r) in predicted_prices.iter().zip(oracle_results.iter()) {
        let leg_acc = acc_score(*v, *r, epsilon_bps);
        if leg_acc > 0 {
            leg_hits += 1;
        }
        total_acc += leg_acc;
    }
    let avg_acc = total_acc.checked_div(leg_count).unwrap_or(0);
    let tb = time_bonus(t_commit, t_open, t_cutoff);
    let wt = if (leg_hits as u64) == leg_count && leg_count > 0 {
        composite_weight(entry_fee, avg_acc, tb)
    } else {
        0
    };
    ParticipantScore {
        leg_hits,
        avg_acc,
        time_bonus: tb,
        composite_weight: wt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: u64 = 103_200;
    const FEE: u64 = 500;
    const OPEN: u64 = 0;
    const CUTOFF: u64 = 1_000_000;

    // Worked example (lightpaper Appendix A, conviction bonus removed since
    // the contract dropped it).

    #[test]
    fn alice_scores() {
        // V=103_150, committed at pool open (T=1.0)
        let acc = acc_score(103_150, R, 600);
        assert_eq!(acc, 996_015_936);
        let tb = time_bonus(0, OPEN, CUTOFF);
        assert_eq!(tb, 2_500_000_000);
        assert_eq!(composite_weight(FEE, acc, tb), 1245);
    }

    #[test]
    fn bob_scores() {
        // V=104_000, committed halfway (T=0.5)
        let acc = acc_score(104_000, R, 600);
        assert_eq!(acc, 928_505_106);
        let tb = time_bonus(500_000, OPEN, CUTOFF);
        assert_eq!(tb, 1_375_000_000);
        assert_eq!(composite_weight(FEE, acc, tb), 638);
    }

    #[test]
    fn carol_scores() {
        // V=98_000, committed late (T=0.05)
        let acc = acc_score(98_000, R, 600);
        assert_eq!(acc, 665_335_994);
        let tb = time_bonus(950_000, OPEN, CUTOFF);
        assert_eq!(tb, 1_003_750_000);
        assert_eq!(composite_weight(FEE, acc, tb), 333);
    }

    #[test]
    fn acc_score_outside_epsilon_is_zero() {
        // Carol's 503 bps error fails a 500 bps tolerance
        assert_eq!(acc_score(98_000, R, 500), 0);
        // exact prediction is perfect
        assert_eq!(acc_score(R, R, 500), FLOAT_SCALING);
    }

    #[test]
    fn time_bonus_clamps() {
        assert_eq!(time_bonus(OPEN, OPEN, CUTOFF), MAX_TIME_BONUS); // at open
        assert_eq!(time_bonus(CUTOFF, OPEN, CUTOFF), FLOAT_SCALING); // at cutoff
        assert_eq!(time_bonus(CUTOFF + 5, OPEN, CUTOFF), FLOAT_SCALING); // past cutoff
        assert_eq!(time_bonus(100, 200, 200), FLOAT_SCALING); // degenerate window
    }

    #[test]
    fn sqrt_fp_known_values() {
        assert_eq!(sqrt_fp(0), 0);
        assert_eq!(sqrt_fp(FLOAT_SCALING), FLOAT_SCALING); // sqrt(1.0) = 1.0
        assert_eq!(sqrt_fp(4 * FLOAT_SCALING), 2 * FLOAT_SCALING); // sqrt(4.0) = 2.0
        assert_eq!(sqrt_fp(250_000_000), 500_000_000); // sqrt(0.25) = 0.5
        assert_eq!(sqrt_fp(2 * FLOAT_SCALING), 1_414_213_562); // sqrt(2) trunc
    }

    #[test]
    fn sqrt_floor_is_exact_floor() {
        for a in [0u128, 1, 2, 3, 4, 15, 16, 17, 1 << 40, (1 << 40) + 1, u64::MAX as u128] {
            let s = sqrt_floor(a);
            assert!(s * s <= a, "a={a}");
            assert!((s + 1) * (s + 1) > a, "a={a}");
        }
        // largest input sqrt_fp can produce: u64::MAX * 1e9
        let a = (u64::MAX as u128) * FLOAT_SCALING_U128;
        let s = sqrt_floor(a);
        assert!(s * s <= a && (s + 1) * (s + 1) > a);
    }

    #[test]
    fn snap_to_grid_basic() {
        // V=61_200, delta=2_100, grid min=50_000 tick=500 max=80_000
        let p = snap_to_grid(61_200, 2_100, 50_000, 500, 80_000).unwrap();
        assert_eq!(p.lower_snapped, 59_000); // floor(59_100)
        assert_eq!(p.upper_snapped, 63_500); // ceil(63_300)
    }

    #[test]
    fn snap_to_grid_errors() {
        assert_eq!(
            snap_to_grid(61_200, 400, 50_000, 500, 80_000),
            Err(MathError::DeltaTooSmall)
        );
        // lower bound underflows the grid
        assert_eq!(
            snap_to_grid(50_100, 500, 50_000, 500, 80_000),
            Err(MathError::BoundsExceeded)
        );
        // upper bound exceeds the grid
        assert_eq!(
            snap_to_grid(79_900, 500, 50_000, 500, 80_000),
            Err(MathError::BoundsExceeded)
        );
    }

    #[test]
    fn svi_atm_iv_flat_smile() {
        // b=0 => implied_var = a; iv = sqrt(a). a=0.04 => iv=0.2
        let iv = svi_atm_iv(
            40_000_000,
            0,
            I64Fp::new(0, false),
            I64Fp::new(0, false),
            0,
        )
        .unwrap();
        assert_eq!(iv, 200_000_000);
    }

    #[test]
    fn svi_atm_iv_with_skew() {
        // a=0.04, b=0.1, rho=-0.3, m=0.1, sigma=0.2
        // m^2 = 0.01, sigma^2 = 0.04 -> sqrt(0.05) = 0.223606797 (floor)
        assert_eq!(sqrt_fp(50_000_000), 223_606_797);
        // -rho*m = +0.03 (rho negative), inner = 0.253606797
        // b*inner = 0.025360679 (truncated), var = 0.065360679
        // iv = floor(sqrt(0.065360679)) = 0.255657346
        let iv = svi_atm_iv(
            40_000_000,
            100_000_000,
            I64Fp::new(300_000_000, true),
            I64Fp::new(100_000_000, false),
            200_000_000,
        )
        .unwrap();
        assert_eq!(iv, 255_657_346);
    }

    #[test]
    fn compute_delta_one_year_atm() {
        // iv=0.5, T = 1 year, V=60_000, k=1.0 => delta = 30_000
        let d = compute_delta(500_000_000, SECONDS_PER_YEAR, 60_000, FLOAT_SCALING);
        assert_eq!(d, 30_000);
        // quarter year: sqrt(0.25)=0.5 => 15_000
        let d = compute_delta(500_000_000, SECONDS_PER_YEAR / 4, 60_000, FLOAT_SCALING);
        assert_eq!(d, 15_000);
    }

    #[test]
    fn parlay_rule_zeroes_weight_on_any_miss() {
        // two legs: first perfect, second misses epsilon entirely
        let s = score_participant(&[R, 200_000], &[R, 100_000], 600, FEE, 0, OPEN, CUTOFF);
        assert_eq!(s.leg_hits, 1);
        assert_eq!(s.composite_weight, 0);
        // avg accuracy still recorded (1e9 + 0) / 2
        assert_eq!(s.avg_acc, FLOAT_SCALING / 2);

        // both legs hit -> weight present
        let s = score_participant(&[R, 100_100], &[R, 100_000], 600, FEE, 0, OPEN, CUTOFF);
        assert_eq!(s.leg_hits, 2);
        assert!(s.composite_weight > 0);
    }
}
