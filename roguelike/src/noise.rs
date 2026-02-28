/// Deterministic procedural noise for terrain generation.
///
/// Uses a hash-based value noise with fractal Brownian motion (fBm) layering.
/// No external dependencies — the hash function is a variant of the
/// Squirrel3 noise hash (GDC 2017), which produces excellent distribution.

/// Seed type for reproducible generation.
pub type NoiseSeed = u64;

/// Squirrel3 positional noise hash.
///
/// Maps an integer position to a pseudo-random `u32`. The bit-mixing
/// constants are chosen to maximize avalanche (every input bit affects
/// every output bit with probability ≈ 0.5).
fn squirrel3(position: i32, seed: u64) -> u32 {
    const NOISE1: u64 = 0xB5297A4D; // large prime with good bit-spread
    const NOISE2: u64 = 0x68E31DA4;
    const NOISE3: u64 = 0x1B56C4E9;

    let mut mangled = position as u64;
    mangled = mangled.wrapping_mul(NOISE1);
    mangled = mangled.wrapping_add(seed);
    mangled ^= mangled >> 8;
    mangled = mangled.wrapping_add(NOISE2);
    mangled ^= mangled << 8;
    mangled = mangled.wrapping_mul(NOISE3);
    mangled ^= mangled >> 8;
    mangled as u32
}

/// 2D noise: combine x and y into a single position before hashing.
fn noise2d(x: i32, y: i32, seed: u64) -> u32 {
    // Combine coordinates via a large prime stride to decorrelate axes.
    let position = x.wrapping_add(y.wrapping_mul(198491));
    squirrel3(position, seed)
}

/// Returns a noise value in [0.0, 1.0) for the given grid coordinate.
pub fn value_noise(x: i32, y: i32, seed: NoiseSeed) -> f64 {
    noise2d(x, y, seed) as f64 / u32::MAX as f64
}

/// Smooth value noise with bilinear interpolation.
///
/// Samples the four lattice corners surrounding the fractional position
/// and interpolates with a smooth Hermite curve (3t² − 2t³) to
/// eliminate grid-axis artifacts.
pub fn smooth_noise(fx: f64, fy: f64, seed: NoiseSeed) -> f64 {
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;

    // Fractional part within the lattice cell.
    let tx = fx - fx.floor();
    let ty = fy - fy.floor();

    // Smoothstep: eliminates first-derivative discontinuities.
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);

    // Four lattice corner values.
    let n00 = value_noise(x0, y0, seed);
    let n10 = value_noise(x0 + 1, y0, seed);
    let n01 = value_noise(x0, y0 + 1, seed);
    let n11 = value_noise(x0 + 1, y0 + 1, seed);

    // Bilinear interpolation with smoothstep.
    let nx0 = n00 + sx * (n10 - n00);
    let nx1 = n01 + sx * (n11 - n01);
    nx0 + sy * (nx1 - nx0)
}

/// Fractal Brownian Motion (fBm) — layers multiple octaves of smooth noise.
///
/// Each successive octave doubles the frequency (`lacunarity = 2.0`)
/// and halves the amplitude (`persistence = 0.5`), producing the
/// characteristic 1/f self-similar pattern found in natural terrain.
///
/// - `octaves`: number of noise layers (4 is a good default).
/// - `frequency`: base spatial frequency (lower = broader features).
/// - `persistence`: amplitude decay per octave.
/// - `seed`: deterministic seed.
pub fn fbm(x: f64, y: f64, octaves: u32, frequency: f64, persistence: f64, seed: NoiseSeed) -> f64 {
    let mut value = 0.0;
    let mut amplitude = 1.0;
    let mut freq = frequency;
    let mut max_amplitude = 0.0;

    for i in 0..octaves {
        // Offset each octave's seed so the layers aren't correlated.
        let octave_seed = seed.wrapping_add(i as u64 * 31);
        value += amplitude * smooth_noise(x * freq, y * freq, octave_seed);
        max_amplitude += amplitude;
        amplitude *= persistence;
        freq *= 2.0; // lacunarity
    }

    // Normalize to [0, 1].
    value / max_amplitude
}
