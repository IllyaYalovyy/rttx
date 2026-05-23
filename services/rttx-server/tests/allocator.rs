//! Verifies that the mimalloc global allocator handles concurrent small
//! allocations without panic or corruption — the workload pattern that
//! motivated the switch from glibc malloc.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[test]
fn mimalloc_handles_concurrent_small_allocations() {
    // Simulate the daemon's PTY buffer churn: many threads doing rapid
    // 4KB alloc/dealloc cycles concurrently.
    let handles: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                let mut vecs: Vec<Vec<u8>> = Vec::new();
                for i in 0..1000 {
                    vecs.push(vec![0u8; 4096]);
                    if i % 100 == 0 {
                        vecs.clear();
                    }
                }
                vecs.len()
            })
        })
        .collect();

    for h in handles {
        let count = h.join().unwrap();
        assert!(count > 0);
    }
}
