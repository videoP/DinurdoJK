//! Bounded, ordered GPU readback for panorama capture, after Render Studio's
//! export slots (`studio_readback_slot` / `finish_studio_readbacks`). One
//! owner records and submits; a slot is reused only after its mapped data was
//! consumed and unmapped. No device-wide idle wait.
use std::{collections::VecDeque, sync::{atomic::{AtomicU8, Ordering}, Arc}, time::{Duration, Instant}};

const MAP_TIMEOUT: Duration = Duration::from_secs(120);

struct InFlight<T> {
    slot: usize,
    tag: T,
    submission: wgpu::SubmissionIndex,
    /// 0 pending, 1 mapped, 2 failed.
    state: Arc<AtomicU8>,
}

pub(super) struct ReadbackRing<T> {
    buffers: Vec<wgpu::Buffer>,
    busy: Vec<bool>,
    inflight: VecDeque<InFlight<T>>,
    size: u64,
    /// Wall time blocked waiting for the oldest readback (not GPU execution time).
    pub(super) wait: Duration,
    /// Wall time inside consumers (unpack/store/hand-off).
    pub(super) consume: Duration,
    pub(super) completed: u64,
}

impl<T> ReadbackRing<T> {
    pub(super) fn new(device: &wgpu::Device, slots: u32, size: u64, label: &str) -> Self {
        let buffers = (0..slots.max(1)).map(|_| device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label), size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        })).collect::<Vec<_>>();
        Self { busy: vec![false; buffers.len()], buffers, inflight: VecDeque::new(), size, wait: Duration::ZERO, consume: Duration::ZERO, completed: 0 }
    }
    pub(super) fn bytes(&self) -> u64 { self.size * self.buffers.len() as u64 }
    pub(super) fn buffer(&self, slot: usize) -> &wgpu::Buffer { &self.buffers[slot] }

    /// A free slot; with all slots in flight the oldest is completed first.
    pub(super) fn acquire(&mut self, device: &wgpu::Device, consume: &mut impl FnMut(T, &[u8]) -> Result<(), String>) -> Result<usize, String> {
        loop {
            if let Some(slot) = self.busy.iter().position(|busy| !busy) { return Ok(slot); }
            self.complete_front(device, consume)?;
        }
    }

    /// Call after the submission that writes `slot` (queue order, no wait).
    pub(super) fn submitted(&mut self, slot: usize, tag: T, submission: wgpu::SubmissionIndex) {
        let state = Arc::new(AtomicU8::new(0));
        let flag = Arc::clone(&state);
        self.buffers[slot].slice(..).map_async(wgpu::MapMode::Read, move |result| {
            flag.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
        });
        self.busy[slot] = true;
        self.inflight.push_back(InFlight { slot, tag, submission, state });
    }

    /// Nonblocking: consume every readback that is already mapped, in order.
    pub(super) fn poll(&mut self, device: &wgpu::Device, consume: &mut impl FnMut(T, &[u8]) -> Result<(), String>) -> Result<(), String> {
        let _ = device.poll(wgpu::PollType::Poll);
        while self.inflight.front().is_some_and(|f| f.state.load(Ordering::Acquire) != 0) {
            self.complete_front(device, consume)?;
        }
        Ok(())
    }

    pub(super) fn drain(&mut self, device: &wgpu::Device, consume: &mut impl FnMut(T, &[u8]) -> Result<(), String>) -> Result<(), String> {
        while !self.inflight.is_empty() { self.complete_front(device, consume)?; }
        Ok(())
    }

    /// Discard a superseded sweep: wait for mappings, unmap, consume nothing.
    pub(super) fn discard(&mut self, device: &wgpu::Device) -> Result<(), String> {
        self.drain(device, &mut |_, _| Ok(()))
    }

    fn complete_front(&mut self, device: &wgpu::Device, consume: &mut impl FnMut(T, &[u8]) -> Result<(), String>) -> Result<(), String> {
        let Some(front) = self.inflight.front() else { return Ok(()); };
        if front.state.load(Ordering::Acquire) == 0 {
            let started = Instant::now();
            device.poll(wgpu::PollType::Wait { submission_index: Some(front.submission.clone()), timeout: Some(MAP_TIMEOUT) })
                .map_err(|e| format!("Screenshot GPU readback wait failed: {e:?}"))?;
            // The map callback runs during that poll; poll once more in case
            // another thread's poll is delivering it concurrently.
            while front.state.load(Ordering::Acquire) == 0 {
                if started.elapsed() > MAP_TIMEOUT { return Err("Screenshot GPU readback timed out".into()); }
                let _ = device.poll(wgpu::PollType::Poll);
                std::thread::yield_now();
            }
            self.wait += started.elapsed();
        }
        let front = self.inflight.pop_front().expect("front exists");
        let buffer = &self.buffers[front.slot];
        if front.state.load(Ordering::Acquire) != 1 {
            self.busy[front.slot] = false;
            return Err("Screenshot GPU readback mapping failed".into());
        }
        let started = Instant::now();
        let result = match buffer.slice(..).get_mapped_range() {
            Ok(view) => { let result = consume(front.tag, &view); drop(view); result }
            Err(error) => Err(format!("Screenshot GPU readback range failed: {error}")),
        };
        buffer.unmap();
        self.busy[front.slot] = false;
        self.consume += started.elapsed();
        self.completed += 1;
        result
    }
}
