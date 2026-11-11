#![allow(unsafe_code)]

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
static AVAILABLE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::arch::is_x86_feature_detected!("avx"));

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[inline]
pub(crate) fn available() -> bool {
    *AVAILABLE
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(crate) fn accumulate_convolution_8(
    z: &mut [f32],
    weights: &[f32],
    history: &[f32],
    base: usize,
    input_channel: usize,
    frames: usize,
) {
    if !available() {
        return;
    }
    // SAFETY: the caller uses this only for C=8 with complete frame groups;
    // AVX support was detected above.
    unsafe { accumulate_convolution_impl(z, weights, history, base, input_channel, frames) }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx")]
unsafe fn accumulate_convolution_impl(
    z: &mut [f32],
    weights: &[f32],
    history: &[f32],
    base: usize,
    input_channel: usize,
    frames: usize,
) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    debug_assert!(weights.len() >= 8);
    debug_assert!(z.len() >= frames * 8);
    debug_assert!(history.len() >= (base + frames) * 8);
    // SAFETY: the caller proves AVX support and eight readable weight lanes.
    let weight = unsafe { _mm256_loadu_ps(weights.as_ptr()) };
    for frame in 0..frames {
        let value = _mm256_set1_ps(history[(base + frame) * 8 + input_channel]);
        // SAFETY: each z group contains eight writable channels.
        let current = unsafe { _mm256_loadu_ps(z.as_ptr().add(frame * 8)) };
        let updated = _mm256_add_ps(current, _mm256_mul_ps(weight, value));
        // SAFETY: the store uses the same in-bounds group as the load.
        unsafe { _mm256_storeu_ps(z.as_mut_ptr().add(frame * 8), updated) };
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(crate) fn accumulate_projection_8(
    projected: &mut [f32],
    z: &[f32],
    weights: &[f32],
    input_channel: usize,
    frames: usize,
) {
    if !available() {
        return;
    }
    // SAFETY: the caller uses this only for C=8 with complete frame groups;
    // AVX support was detected above.
    unsafe { accumulate_projection_impl(projected, z, weights, input_channel, frames) }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx")]
unsafe fn accumulate_projection_impl(
    projected: &mut [f32],
    z: &[f32],
    weights: &[f32],
    input_channel: usize,
    frames: usize,
) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    debug_assert!(weights.len() >= 8);
    debug_assert!(projected.len() >= frames * 8 && z.len() >= frames * 8);
    // SAFETY: the caller proves AVX support and eight readable weight lanes.
    let weight = unsafe { _mm256_loadu_ps(weights.as_ptr()) };
    for frame in 0..frames {
        // SAFETY: the projected frame group contains eight writable lanes.
        let current = unsafe { _mm256_loadu_ps(projected.as_ptr().add(frame * 8)) };
        let input = _mm256_set1_ps(z[frame * 8 + input_channel]);
        let updated = _mm256_add_ps(current, _mm256_mul_ps(weight, input));
        // SAFETY: the output group is writable and has the same eight lanes.
        unsafe { _mm256_storeu_ps(projected.as_mut_ptr().add(frame * 8), updated) };
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(crate) fn activate_8(
    z: &mut [f32],
    head_sum: &mut [f32],
    mixin_weights: &[f32],
    cond: &[f32],
    frames: usize,
) {
    if !available() {
        return;
    }
    // SAFETY: the caller uses this only for C=8 with complete frame groups;
    // AVX support was detected above.
    unsafe { activate_impl(z, head_sum, mixin_weights, cond, frames) }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx")]
unsafe fn activate_impl(
    z: &mut [f32],
    head_sum: &mut [f32],
    mixin_weights: &[f32],
    cond: &[f32],
    frames: usize,
) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    // SAFETY: the caller supplies eight readable mixin weights.
    let mixin = unsafe { _mm256_loadu_ps(mixin_weights.as_ptr()) };
    let zero = _mm256_setzero_ps();
    let negative_scale = _mm256_set1_ps(0.01);
    for (frame, condition) in cond.iter().enumerate().take(frames) {
        let condition = _mm256_set1_ps(*condition);
        // SAFETY: every frame group contains eight readable/writable lanes.
        let (current, sum) = unsafe {
            (
                _mm256_loadu_ps(z.as_ptr().add(frame * 8)),
                _mm256_loadu_ps(head_sum.as_ptr().add(frame * 8)),
            )
        };
        let mixed = _mm256_add_ps(current, _mm256_mul_ps(mixin, condition));
        let negative = _mm256_cmp_ps::<_CMP_LT_OQ>(mixed, zero);
        let activated = _mm256_blendv_ps(mixed, _mm256_mul_ps(mixed, negative_scale), negative);
        // SAFETY: both stores use the same in-bounds groups as the loads.
        unsafe {
            _mm256_storeu_ps(z.as_mut_ptr().add(frame * 8), activated);
            _mm256_storeu_ps(
                head_sum.as_mut_ptr().add(frame * 8),
                _mm256_add_ps(sum, activated),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn activation_matches_scalar_bits() {
        let frames = 16;
        let mut z = (0..frames * 8)
            .map(|index| (index as f32 - 63.0) * 0.03125)
            .collect::<Vec<_>>();
        let mut head_sum = (0..frames * 8)
            .map(|index| (index as f32 - 47.0) * 0.0125)
            .collect::<Vec<_>>();
        let mixin = (0..8)
            .map(|index| (index as f32 - 3.0) * 0.25)
            .collect::<Vec<_>>();
        let cond = (0..frames)
            .map(|index| (index as f32 - 7.0) * 0.125)
            .collect::<Vec<_>>();
        let mut expected_z = z.clone();
        let mut expected_sum = head_sum.clone();
        for (frame, condition) in cond.iter().enumerate() {
            for (channel, weight) in mixin.iter().enumerate() {
                let index = frame * 8 + channel;
                let mixed = expected_z[index] + weight * condition;
                expected_z[index] = if mixed >= 0.0 { mixed } else { mixed * 0.01 };
                expected_sum[index] += expected_z[index];
            }
        }
        super::activate_8(&mut z, &mut head_sum, &mixin, &cond, frames);
        assert_eq!(
            z.iter().map(|value| value.to_bits()).collect::<Vec<_>>(),
            expected_z
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            head_sum
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            expected_sum
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
    }
}
