//! Cache.
use crate::scene::{
    active_game_directory, DrawBatch, GpuVertex, Path, PathBuf, Pod, STATIC_BSP_AO_CACHE_VERSION,
};

#[derive(Debug, Clone)]
pub struct StaticBspAoCacheInfo {
    pub directory: PathBuf,
    pub map_name: String,
    pub map_hash: u64,
    pub version: u32,
}

pub(in crate::scene) fn map_content_hash(bytes: &[u8]) -> u64 {
    // Deterministic FNV-1a: unlike DefaultHasher this remains stable across runs.
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Fingerprints of the option-dependent stage inputs a prepared map was built
/// from. A later preparation of the same map compares its own inputs against
/// these and, on a match, copies the finished result out of the previous map
/// (the app's restart cache) instead of recomputing it. A hit is therefore
/// bit-identical to a rebuild; a changed input recomputes only that stage.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrepStageKeys {
    pub grass: Option<u64>,
    pub gi: Option<u64>,
    pub portal: Option<u64>,
}

/// Input fingerprint for a reusable stage. `DefaultHasher::new()` is
/// deterministic within a process, which is all an in-memory comparison needs.
pub(in crate::scene) struct StageFingerprint(FoldHasher);

/// 64-bit multiply-fold hasher (wyhash style): one 128-bit multiply per eight
/// bytes. Fingerprints only ever compare against another fingerprint made by
/// this process, so speed matters and stability across builds does not; SipHash
/// was the bulk of the time spent keying stages over millions of vertices.
#[derive(Default)]
pub(crate) struct FoldHasher {
    pub(in crate::scene) state: u64,
    pub(in crate::scene) length: u64,
}

impl FoldHasher {
    pub(in crate::scene) const K1: u64 = 0x9e37_79b9_7f4a_7c15;
    pub(in crate::scene) const K2: u64 = 0xd6e8_feb8_6659_fd93;

    #[inline]
    pub(in crate::scene) fn round(&mut self, word: u64) {
        let product = u128::from(self.state ^ word ^ Self::K1) * u128::from(Self::K2);
        self.state = (product as u64) ^ ((product >> 64) as u64);
    }
}

impl std::hash::Hasher for FoldHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.length = self.length.wrapping_add(bytes.len() as u64);
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.round(u64::from_le_bytes(
                chunk.try_into().expect("chunk of eight"),
            ));
        }
        let tail = chunks.remainder();
        if !tail.is_empty() {
            let mut padded = [0_u8; 8];
            padded[..tail.len()].copy_from_slice(tail);
            // The tail length keeps "ab" and "ab\0" apart.
            self.round(u64::from_le_bytes(padded) ^ ((tail.len() as u64) << 59));
        }
    }

    fn finish(&self) -> u64 {
        let product = u128::from(self.state ^ self.length) * u128::from(Self::K2);
        (product as u64) ^ ((product >> 64) as u64)
    }
}

impl StageFingerprint {
    pub(in crate::scene) fn new(stage: &str) -> Self {
        use std::hash::Hasher;
        let mut hasher = FoldHasher::default();
        hasher.write(stage.as_bytes());
        Self(hasher)
    }

    /// Hash draw batches. Their `Debug` text is cheap except for the PVS
    /// signature (dozens of words per batch), so each signature is set aside,
    /// hashed as raw bytes, and put back.
    pub(in crate::scene) fn draw_batches(&mut self, batches: &mut [DrawBatch]) -> &mut Self {
        use std::hash::Hasher;
        self.0.write_usize(batches.len());
        for batch in batches {
            let signature = std::mem::take(&mut batch.pvs_signature);
            self.debug(&*batch);
            self.pod(&signature);
            batch.pvs_signature = signature;
        }
        self
    }

    /// Hash vertex positions only, in blocks so the hasher sees large writes.
    pub(in crate::scene) fn positions(&mut self, vertices: &[GpuVertex]) -> &mut Self {
        let mut block = [[0.0_f32; 3]; 512];
        for chunk in vertices.chunks(block.len()) {
            for (slot, vertex) in block.iter_mut().zip(chunk) {
                *slot = vertex.position;
            }
            self.pod(&block[..chunk.len()]);
        }
        self
    }

    pub(in crate::scene) fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        use std::hash::Hasher;
        self.0.write_usize(bytes.len());
        self.0.write(bytes);
        self
    }

    pub(in crate::scene) fn pod<T: Pod>(&mut self, values: &[T]) -> &mut Self {
        self.bytes(bytemuck::cast_slice(values))
    }

    /// Hash a value through its `Debug` text without allocating it.
    pub(in crate::scene) fn debug<T: std::fmt::Debug + ?Sized>(&mut self, value: &T) -> &mut Self {
        use std::fmt::Write;
        let _ = write!(self, "{value:?}");
        self
    }

    pub(in crate::scene) fn finish(&self) -> u64 {
        use std::hash::Hasher;
        self.0.finish()
    }
}

impl std::fmt::Write for StageFingerprint {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        use std::hash::Hasher;
        self.0.write(text.as_bytes());
        Ok(())
    }
}

pub(in crate::scene) fn static_bsp_ao_cache_info(
    root: &Path,
    game: Option<&Path>,
    name: &str,
    map_hash: u64,
) -> StaticBspAoCacheInfo {
    StaticBspAoCacheInfo {
        directory: active_game_directory(root, game)
            .join("jka-rust-cache")
            .join("static-bsp-ao"),
        map_name: name.to_string(),
        map_hash,
        version: STATIC_BSP_AO_CACHE_VERSION,
    }
}
