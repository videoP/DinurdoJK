//! Visibility readback.
use crate::renderer::{
    mpsc, Receiver, TryRecvError, CULL_DIAGNOSTIC_COUNTER_BYTES, CULL_DIAGNOSTIC_READBACK_SLOTS,
};

pub(in crate::renderer) struct CullDiagnosticReadbackSlot {
    pub(in crate::renderer) readback_buffer: wgpu::Buffer,
    pub(in crate::renderer) pending: Option<Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

pub(in crate::renderer) struct CullDiagnosticsReadback {
    pub(in crate::renderer) slots: Vec<CullDiagnosticReadbackSlot>,
    pub(in crate::renderer) current: Option<usize>,
    pub(in crate::renderer) encoded: bool,
    pub(in crate::renderer) latest: [u32; 4],
}

impl CullDiagnosticsReadback {
    pub(in crate::renderer) fn new(device: &wgpu::Device) -> Self {
        let slots = (0..CULL_DIAGNOSTIC_READBACK_SLOTS)
            .map(|_| CullDiagnosticReadbackSlot {
                readback_buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("JKA cull diagnostic readback"),
                    size: CULL_DIAGNOSTIC_COUNTER_BYTES,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                pending: None,
            })
            .collect();
        Self {
            slots,
            current: None,
            encoded: false,
            latest: [0; 4],
        }
    }

    pub(in crate::renderer) fn begin_frame(&mut self, device: &wgpu::Device) {
        if !self.slots.iter().any(|slot| slot.pending.is_some()) {
            self.current = self.slots.iter().position(|slot| slot.pending.is_none());
            self.encoded = false;
            return;
        }
        let _ = device.poll(wgpu::PollType::Poll);
        for slot in &mut self.slots {
            let completed = {
                let Some(pending) = slot.pending.as_ref() else {
                    continue;
                };
                match pending.try_recv() {
                    Ok(Ok(())) => {
                        let bytes = slot.readback_buffer.slice(..).get_mapped_range();
                        let values: &[u32] = bytemuck::cast_slice(&bytes);
                        let latest = [
                            values.first().copied().unwrap_or(0),
                            values.get(1).copied().unwrap_or(0),
                            values.get(2).copied().unwrap_or(0),
                            values.get(3).copied().unwrap_or(0),
                        ];
                        drop(bytes);
                        slot.readback_buffer.unmap();
                        Some(latest)
                    }
                    Ok(Err(_)) | Err(TryRecvError::Disconnected) => None,
                    Err(TryRecvError::Empty) => continue,
                }
            };
            slot.pending = None;
            if let Some(latest) = completed {
                self.latest = latest;
            }
        }
        self.current = self.slots.iter().position(|slot| slot.pending.is_none());
        self.encoded = false;
    }

    pub(in crate::renderer) fn encode_copy(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Buffer,
    ) {
        let Some(current) = self.current else {
            return;
        };
        encoder.copy_buffer_to_buffer(
            source,
            0,
            &self.slots[current].readback_buffer,
            0,
            CULL_DIAGNOSTIC_COUNTER_BYTES,
        );
        self.encoded = true;
    }

    pub(in crate::renderer) fn submit(&mut self) {
        if !self.encoded {
            self.current = None;
            return;
        }
        let Some(current) = self.current.take() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.slots[current].readback_buffer.slice(..).map_async(
            wgpu::MapMode::Read,
            move |result| {
                let _ = sender.send(result);
            },
        );
        self.slots[current].pending = Some(receiver);
        self.encoded = false;
    }
}
