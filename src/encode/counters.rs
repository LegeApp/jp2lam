use std::sync::atomic::{AtomicU64, Ordering};

pub static TOTAL_BLOCKS: AtomicU64 = AtomicU64::new(0);
pub static EMPTY_BLOCKS: AtomicU64 = AtomicU64::new(0);
pub static MQ_SYMBOLS: AtomicU64 = AtomicU64::new(0);
pub static CLEANUP_PASSES: AtomicU64 = AtomicU64::new(0);
pub static SP_PASSES: AtomicU64 = AtomicU64::new(0);
pub static MR_PASSES: AtomicU64 = AtomicU64::new(0);
pub static TOTAL_PASS_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn reset() {
    TOTAL_BLOCKS.store(0, Ordering::Relaxed);
    EMPTY_BLOCKS.store(0, Ordering::Relaxed);
    MQ_SYMBOLS.store(0, Ordering::Relaxed);
    CLEANUP_PASSES.store(0, Ordering::Relaxed);
    SP_PASSES.store(0, Ordering::Relaxed);
    MR_PASSES.store(0, Ordering::Relaxed);
    TOTAL_PASS_BYTES.store(0, Ordering::Relaxed);
}

pub fn print() {
    let total = TOTAL_BLOCKS.load(Ordering::Relaxed);
    let empty = EMPTY_BLOCKS.load(Ordering::Relaxed);
    let mq = MQ_SYMBOLS.load(Ordering::Relaxed);
    let cl = CLEANUP_PASSES.load(Ordering::Relaxed);
    let sp = SP_PASSES.load(Ordering::Relaxed);
    let mr = MR_PASSES.load(Ordering::Relaxed);
    let bytes = TOTAL_PASS_BYTES.load(Ordering::Relaxed);
    
    let nonempty = total.saturating_sub(empty);
    let total_passes = cl.saturating_add(sp).saturating_add(mr);
    
    println!("\n=== Tier-1 Counters ===");
    println!("  Blocks: total={} empty={} ({:.1}%)", 
        total, empty, if total > 0 { 100.0*empty as f64/total as f64 } else { 0.0 });
    println!("  Passes: cleanup={} SP={} MR={} (total={})", cl, sp, mr, total_passes);
    println!("  MQ symbols: {} ({:.1} per block, {:.1} per pass)", 
        mq, if total > 0 { mq as f64/total as f64 } else { 0.0 },
        if total_passes > 0 { mq as f64/total_passes as f64 } else { 0.0 });
    println!("  Bytes: {} (avg {:.1} per pass)", bytes, if total_passes > 0 { bytes as f64/total_passes as f64 } else { 0.0 });
}