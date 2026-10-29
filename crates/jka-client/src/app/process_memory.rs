//! `memory_info` breakdown of the process's private committed memory: which
//! kind of region holds it and how much of it is actually resident. Commit far
//! above the working set means reserved-but-untouched buffers or driver-backed
//! mappings, which this separates without guessing at owners.

#[cfg(windows)]
mod ffi {
    #![allow(non_snake_case, clippy::upper_case_acronyms)]
    // Declared here so no extra windows-sys feature (and dependency rebuild)
    // is needed for a diagnostic.
    #[repr(C)]
    #[derive(Default)]
    pub struct MemoryBasicInformation {
        pub BaseAddress: usize,
        pub AllocationBase: usize,
        pub AllocationProtect: u32,
        pub PartitionId: u16,
        pub RegionSize: usize,
        pub State: u32,
        pub Protect: u32,
        pub Type: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct WorkingSetExInformation {
        pub VirtualAddress: usize,
        pub VirtualAttributes: usize,
    }
    pub const MEM_COMMIT: u32 = 0x1000;
    pub const MEM_PRIVATE: u32 = 0x20000;
    pub const PAGE_NOCACHE: u32 = 0x200;
    pub const PAGE_WRITECOMBINE: u32 = 0x400;
    pub const PAGE_GUARD: u32 = 0x100;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn VirtualQuery(address: *const core::ffi::c_void, buffer: *mut MemoryBasicInformation, length: usize) -> usize;
        pub fn GetCurrentProcess() -> *mut core::ffi::c_void;
        pub fn K32QueryWorkingSetEx(process: *mut core::ffi::c_void, buffer: *mut WorkingSetExInformation, size: u32) -> i32;
    }
}

#[cfg(windows)]
pub(super) fn report_lines() -> Vec<String> {
    use ffi::*;
    use std::collections::HashMap;
    let mib = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
    // allocation base -> (committed bytes, protection of its largest region, committed ranges)
    let mut allocations: HashMap<usize, (usize, u32, Vec<(usize, usize)>)> = HashMap::new();
    let (mut read_write, mut write_combine, mut other) = (0usize, 0usize, 0usize);
    let mut address = 0usize;
    loop {
        let mut info = MemoryBasicInformation::default();
        // SAFETY: VirtualQuery only reads the address space map into `info`.
        let written = unsafe {
            VirtualQuery(address as *const _, &mut info, std::mem::size_of::<MemoryBasicInformation>())
        };
        if written == 0 || info.RegionSize == 0 {
            break;
        }
        if info.State == MEM_COMMIT && info.Type == MEM_PRIVATE && info.Protect & PAGE_GUARD == 0 {
            if info.Protect & (PAGE_WRITECOMBINE | PAGE_NOCACHE) != 0 {
                write_combine += info.RegionSize;
            } else if info.Protect & 0x04 != 0 {
                read_write += info.RegionSize;
            } else {
                other += info.RegionSize;
            }
            let entry = allocations.entry(info.AllocationBase).or_insert((0, info.Protect, Vec::new()));
            entry.0 += info.RegionSize;
            entry.2.push((info.BaseAddress, info.RegionSize));
        }
        let Some(next) = info.BaseAddress.checked_add(info.RegionSize) else { break };
        address = next;
    }
    let total = read_write + write_combine + other;
    let mut lines = vec![format!(
        "MEMORY process regions: private committed {:.2} MiB = read/write {:.2} + write-combined/uncached (driver mappings) {:.2} + other {:.2}",
        mib(total), mib(read_write), mib(write_combine), mib(other),
    )];

    // No HeapSummary/HeapWalk: walking heaps other threads are using is the
    // suspected cause of a crash right after this report first ran. The
    // read/write share above is the heap-backed (Rust and C runtime) memory.

    let mut buckets = [(0usize, 0usize); 4];
    for (committed, _, _) in allocations.values() {
        let bucket = match *committed {
            size if size < 1 << 20 => 0,
            size if size < 16 << 20 => 1,
            size if size < 256 << 20 => 2,
            _ => 3,
        };
        buckets[bucket].0 += 1;
        buckets[bucket].1 += committed;
    }
    lines.push(format!(
        "MEMORY process allocation sizes: <1 MiB {} = {:.2} MiB; 1-16 MiB {} = {:.2} MiB; 16-256 MiB {} = {:.2} MiB; >=256 MiB {} = {:.2} MiB",
        buckets[0].0, mib(buckets[0].1), buckets[1].0, mib(buckets[1].1),
        buckets[2].0, mib(buckets[2].1), buckets[3].0, mib(buckets[3].1),
    ));

    // Largest allocations with a sampled resident share (QueryWorkingSetEx).
    let mut largest: Vec<_> = allocations.into_iter().collect();
    largest.sort_unstable_by_key(|(_, (committed, _, _))| std::cmp::Reverse(*committed));
    const PAGE: usize = 4096;
    const SAMPLES: usize = 512;
    for (base, (committed, protect, ranges)) in largest.into_iter().take(12) {
        let pages = committed / PAGE;
        let stride = (pages / SAMPLES).max(1);
        let mut samples = Vec::with_capacity(SAMPLES);
        let mut index = 0usize;
        'ranges: for (start, size) in &ranges {
            let mut offset = 0usize;
            while offset < *size {
                if index % stride == 0 {
                    samples.push(WorkingSetExInformation { VirtualAddress: start + offset, VirtualAttributes: 0 });
                    if samples.len() == SAMPLES { break 'ranges; }
                }
                index += 1;
                offset += PAGE;
            }
        }
        // SAFETY: the buffer holds `samples.len()` initialized entries.
        let ok = unsafe {
            K32QueryWorkingSetEx(GetCurrentProcess(), samples.as_mut_ptr(),
                (samples.len() * std::mem::size_of::<WorkingSetExInformation>()) as u32)
        } != 0;
        let resident = if ok && !samples.is_empty() {
            let valid = samples.iter().filter(|s| s.VirtualAttributes & 1 != 0).count();
            format!("{:.0}% resident", valid as f64 * 100.0 / samples.len() as f64)
        } else {
            "residency unknown".to_owned()
        };
        lines.push(format!(
            "MEMORY process allocation: {:.2} MiB at {base:#x}, protect {protect:#x}, {} region(s), {resident}",
            mib(committed), ranges.len(),
        ));
    }
    lines
}

#[cfg(not(windows))]
pub(super) fn report_lines() -> Vec<String> {
    Vec::new()
}
