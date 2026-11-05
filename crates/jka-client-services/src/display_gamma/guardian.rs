//! Durable per-monitor restoration state and serial gamma operations.
use std::{collections::HashMap, fs, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Ramp(pub [u16; 768]);
impl Ramp {
    pub(super) fn valid(&self) -> bool {
        self.0
            .chunks_exact(256)
            .all(|channel| channel.iter().max() > channel.iter().min())
    }
    pub(super) fn close_to(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(&other.0)
            .all(|(a, b)| a.abs_diff(*b) <= 256)
    }
    pub(super) fn with_gamma(&self, gamma: f32) -> Self {
        if gamma == 1.0 {
            return self.clone();
        }
        let mut ramp = self.clone();
        for channel in 0..3 {
            for i in 0..256 {
                let position = (i as f32 / 255.0).powf(1.0 / gamma) * 255.0;
                let lower = position.floor() as usize;
                let upper = (lower + 1).min(255);
                let t = position - lower as f32;
                let a = self.0[channel * 256 + lower] as f32;
                let b = self.0[channel * 256 + upper] as f32;
                ramp.0[channel * 256 + i] = (a + (b - a) * t).round() as u16;
            }
        }
        ramp
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Device {
    pub name: String,
    pub identity: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Owner {
    pub pid: u32,
    pub born: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub device: Device,
    pub owner: Owner,
    pub original: Ramp,
    pub applied: Ramp,
    pub previous: Ramp,
}
impl Snapshot {
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut bytes = b"JKGAMMA1".to_vec();
        bytes.extend(self.owner.pid.to_le_bytes());
        bytes.extend(self.owner.born.to_le_bytes());
        for value in [&self.device.name, &self.device.identity] {
            bytes.extend((value.len() as u32).to_le_bytes());
            bytes.extend(value.as_bytes());
        }
        for ramp in [&self.original, &self.applied, &self.previous] {
            for value in ramp.0 {
                bytes.extend(value.to_le_bytes());
            }
        }
        bytes.extend(checksum(&bytes).to_le_bytes());
        bytes
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 28 || &bytes[..8] != b"JKGAMMA1" {
            return Err("invalid gamma recovery file".into());
        }
        let last = bytes.len() - 4;
        if checksum(&bytes[..last]) != u32::from_le_bytes(bytes[last..].try_into().unwrap()) {
            return Err("gamma recovery checksum mismatch".into());
        }
        let mut cursor = 8;
        fn take<'a>(bytes: &'a [u8], cursor: &mut usize, n: usize) -> Result<&'a [u8], String> {
            let value = bytes
                .get(*cursor..cursor.saturating_add(n))
                .ok_or("truncated gamma recovery file")?;
            *cursor += n;
            Ok(value)
        }
        let pid = u32::from_le_bytes(take(bytes, &mut cursor, 4)?.try_into().unwrap());
        let born = u64::from_le_bytes(take(bytes, &mut cursor, 8)?.try_into().unwrap());
        let mut strings = Vec::new();
        for _ in 0..2 {
            let len = u32::from_le_bytes(take(bytes, &mut cursor, 4)?.try_into().unwrap()) as usize;
            if len > 4096 {
                return Err("oversized gamma recovery device name".into());
            }
            strings.push(
                String::from_utf8(take(bytes, &mut cursor, len)?.to_vec())
                    .map_err(|e| e.to_string())?,
            );
        }
        let mut ramps = Vec::new();
        for _ in 0..3 {
            let packed = take(bytes, &mut cursor, 768 * 2)?;
            let ramp = Ramp(std::array::from_fn(|i| {
                u16::from_le_bytes([packed[i * 2], packed[i * 2 + 1]])
            }));
            if !ramp.valid() {
                return Err("invalid gamma recovery curve".into());
            }
            ramps.push(ramp);
        }
        if cursor != last {
            return Err("unexpected gamma recovery trailing data".into());
        }
        Ok(Self {
            device: Device {
                name: strings.remove(0),
                identity: strings.remove(0),
            },
            owner: Owner { pid, born },
            original: ramps.remove(0),
            applied: ramps.remove(0),
            previous: ramps.remove(0),
        })
    }
}
fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(16777619)
    })
}
pub(super) fn device_key(device: &Device) -> String {
    format!(
        "{:08x}-{:08x}",
        checksum(device.name.as_bytes()),
        checksum(device.identity.as_bytes())
    )
}
pub(super) trait Backend {
    type Lease;
    fn owner(&self) -> Owner;
    fn lock(&self, device: &Device) -> Result<Self::Lease, String>;
    fn window_device(&self, hwnd: usize) -> Result<Device, String>;
    fn read(&self, device: &Device) -> Result<Ramp, String>;
    fn write(&self, device: &Device, ramp: &Ramp) -> Result<(), String>;
}
struct Saved<L> {
    snapshot: Snapshot,
    _lease: L,
}
pub(super) struct Keeper<B: Backend> {
    pub backend: B,
    directory: PathBuf,
    saved: HashMap<String, Saved<B::Lease>>,
}
impl<B: Backend> Keeper<B> {
    pub(super) fn new(backend: B, directory: PathBuf) -> Self {
        Self {
            backend,
            directory,
            saved: HashMap::new(),
        }
    }
    fn path(&self, key: &str) -> PathBuf {
        self.directory.join(format!("{key}.gamma"))
    }
    fn persist(&self, key: &str, snapshot: &Snapshot) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        super::atomic_write(&self.path(key), &snapshot.encode())
    }
    /// Called before any new baseline is captured. A named monitor lease prevents
    /// another running game/guardian from having its ramp mistaken for a desktop baseline.
    pub(super) fn recover(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        let files = match fs::read_dir(&self.directory) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return errors,
            Err(error) => return vec![format!("read gamma recovery directory: {error}")],
        };
        for file in files {
            let file = match file {
                Ok(file) => file,
                Err(error) => {
                    errors.push(format!("read gamma recovery entry: {error}"));
                    continue;
                }
            };
            let path = file.path();
            if path.extension().and_then(|v| v.to_str()) != Some("gamma") {
                continue;
            }
            let result = (|| {
                let bytes = fs::read(&path).map_err(|e| e.to_string())?;
                let snapshot = Snapshot::decode(&bytes)?;
                let _lease = self.backend.lock(&snapshot.device)?;
                self.restore_snapshot(&snapshot)?;
                fs::remove_file(&path).map_err(|e| e.to_string())
            })();
            if let Err(error) = result {
                errors.push(format!("{}: {error}", path.display()));
            }
        }
        errors
    }
    fn restore_snapshot(&self, snapshot: &Snapshot) -> Result<(), String> {
        let current = self.backend.read(&snapshot.device)?;
        if current == snapshot.original
            && snapshot.applied == snapshot.original
            && snapshot.previous == snapshot.original
        {
            return Ok(()); // backup captured, but no gamma change was attempted
        }
        if current != snapshot.original
            && !current.close_to(&snapshot.applied)
            && !current.close_to(&snapshot.previous)
        {
            return Err("display gamma changed outside the game; retained recovery backup".into());
        }
        self.backend.write(&snapshot.device, &snapshot.original)?;
        if !self
            .backend
            .read(&snapshot.device)?
            .close_to(&snapshot.original)
        {
            return Err(
                "driver did not restore original display gamma; retained recovery backup".into(),
            );
        }
        Ok(())
    }
    pub(super) fn restore_all(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        let keys: Vec<_> = self.saved.keys().cloned().collect();
        for key in keys {
            let saved = &self.saved[&key];
            match self.restore_snapshot(&saved.snapshot) {
                Ok(()) => match fs::remove_file(self.path(&key)) {
                    Ok(()) => {
                        self.saved.remove(&key);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        self.saved.remove(&key);
                    }
                    Err(error) => errors.push(error.to_string()),
                },
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
    pub(super) fn apply(&mut self, hwnd: usize, gamma: f32) -> Result<(), String> {
        if !gamma.is_finite() || !(0.5..=3.0).contains(&gamma) {
            return Err("invalid hardware gamma".into());
        }
        let device = self.backend.window_device(hwnd)?;
        let key = device_key(&device);
        if self.saved.keys().any(|other| other != &key) {
            self.restore_all()?;
        }
        if !self.saved.contains_key(&key) {
            let lease = self.backend.lock(&device)?;
            let path = self.path(&key);
            if path.exists() {
                let old = Snapshot::decode(&fs::read(&path).map_err(|e| e.to_string())?)?;
                if old.device != device {
                    return Err("gamma recovery monitor identity mismatch".into());
                }
                self.restore_snapshot(&old)?;
                fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
            let original = self.backend.read(&device)?;
            if !original.valid() {
                return Err("driver returned an invalid desktop gamma ramp".into());
            }
            let snapshot = Snapshot {
                device,
                owner: self.backend.owner(),
                applied: original.clone(),
                previous: original.clone(),
                original,
            };
            self.persist(&key, &snapshot)?; // durable before the first hardware write
            self.saved.insert(
                key.clone(),
                Saved {
                    snapshot,
                    _lease: lease,
                },
            );
        }
        let mut next = self.saved[&key].snapshot.clone();
        let current = self.backend.read(&next.device)?;
        if !current.close_to(&next.applied) && !current.close_to(&next.original) {
            return Err(
                "another application changed display gamma; hardware mode suspended".into(),
            );
        }
        next.previous = current;
        next.applied = next.original.with_gamma(gamma);
        self.persist(&key, &next)?; // crash between write and readback is recoverable
        self.saved.get_mut(&key).unwrap().snapshot = next.clone();
        self.backend.write(&next.device, &next.applied)?;
        let actual = self.backend.read(&next.device)?;
        if !actual.close_to(&next.applied) {
            // Keep a non-neutral intended curve if a driver reports the baseline
            // after a write: restore must still explicitly write the original.
            if actual != next.original {
                next.applied = actual;
                self.persist(&key, &next)?;
                self.saved.get_mut(&key).unwrap().snapshot = next;
            }
            return Err("driver ignored or altered hardware gamma".into());
        }
        next.applied = actual;
        self.persist(&key, &next)?;
        self.saved.get_mut(&key).unwrap().snapshot = next;
        Ok(())
    }
}
impl<B: Backend> Drop for Keeper<B> {
    fn drop(&mut self) {
        let _ = self.restore_all();
    }
}

#[cfg(test)]
pub(super) fn recovery_path(root: &std::path::Path, device: &Device) -> PathBuf {
    root.join(format!("{}.gamma", device_key(device)))
}
