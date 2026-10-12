//! Timings of the two most used filters on a large plane. Ignored by
//! default; run with
//! `cargo test -p calipix-ops --test filter_timing -- --ignored --nocapture`.

use std::time::Instant;

use calipix_core::Buffer;
use calipix_ops::filter::{Frame, Rank, gaussian_blur, rank_filter};

#[test]
#[ignore = "a timing, not a check"]
fn gaussian_and_median_on_a_2048_square_16_bit_plane() {
    let n = 2048;
    let samples: Vec<u16> = (0..n * n)
        .map(|i| ((i as u64 * 2_654_435_761) >> 7) as u16)
        .collect();
    let plane = Buffer::from_vec(samples);
    let frame = Frame::whole(n, n);
    println!("threads: {}", rayon::current_num_threads());
    for round in 0..2 {
        let start = Instant::now();
        let blurred = gaussian_blur(&plane, frame, 2.0, 2.0, 0.0002).unwrap();
        let gaussian = start.elapsed();
        let start = Instant::now();
        let median = rank_filter(&plane, frame, 2.0, Rank::Median).unwrap();
        println!(
            "round {round}: Gaussian sigma 2: {gaussian:.2?}; Median radius 2: {:.2?}",
            start.elapsed()
        );
        assert_eq!(blurred.len(), median.len());
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    pool.install(|| {
        let start = Instant::now();
        gaussian_blur(&plane, frame, 2.0, 2.0, 0.0002).unwrap();
        let gaussian = start.elapsed();
        let start = Instant::now();
        rank_filter(&plane, frame, 2.0, Rank::Median).unwrap();
        println!(
            "one thread: Gaussian sigma 2: {gaussian:.2?}; Median radius 2: {:.2?}",
            start.elapsed()
        );
    });
}
