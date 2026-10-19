//! CPU datasets and cooked curve bindings for the original VectorVM ABI.
//! System/emitter scheduling and renderer binding are supplied by the caller.
use crate::vector_vm::{External, Host, Kernel, Numeric, RandomStream};
use serde_json::Value;
use std::collections::BTreeMap;
const MAX_INSTANCES: usize = 100_000;
const MAX_COLUMNS: usize = 2048;

#[derive(Default, Debug)]
pub struct Dataset {
    pub input: BTreeMap<(Numeric, u16), Vec<u32>>,
    pub output: BTreeMap<(Numeric, u16), Vec<u32>>,
    pub output_instances: usize,
    ids: BTreeMap<u32, u32>,
    free_ids: Vec<u32>,
    next_id: u32,
    acquire_tag: u32,
}
impl Dataset {
    pub fn begin_pass(&mut self) {
        self.output.clear();
        self.output_instances = 0;
        self.acquire_tag = self.acquire_tag.wrapping_add(1);
    }
    pub fn commit(&mut self) {
        self.input = std::mem::take(&mut self.output);
    }
    pub fn id_instance(&self, id: u32) -> Option<u32> {
        self.ids.get(&id).copied()
    }
}
#[derive(Debug)]
pub struct Curve {
    channels: usize,
    samples: Vec<f32>,
    min_time: f32,
    inverse_range: f32,
}
impl Curve {
    pub fn from_export(object: &Value) -> Result<Self, String> {
        let channels = match object["Type"].as_str() {
            Some("NiagaraDataInterfaceCurve") => 1,
            Some("NiagaraDataInterfaceVector2DCurve") => 2,
            Some("NiagaraDataInterfaceVectorCurve") => 3,
            Some("NiagaraDataInterfaceColorCurve") => 4,
            _ => return Err("not a cooked Niagara curve data interface".into()),
        };
        let p = &object["Properties"];
        // Cooked exports omit default properties: a curve without keys has an
        // empty LUT and evaluates to zero in every channel.
        if p["ShaderLUT"].is_null() {
            return Ok(Self {
                channels,
                samples: vec![0.; channels],
                min_time: 0.,
                inverse_range: 0.,
            });
        }
        let samples = p["ShaderLUT"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 65536 * channels && a.len() % channels == 0)
            .ok_or("invalid Niagara curve LUT")?
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|n| n as f32)
                    .filter(|n| n.is_finite())
                    .ok_or("non-finite Niagara curve sample".into())
            })
            .collect::<Result<Vec<_>, String>>()?;
        let min_time = p["LUTMinTime"].as_f64().unwrap_or(0.) as f32;
        let max_time = p["LUTMaxTime"].as_f64().unwrap_or(1.) as f32;
        if !min_time.is_finite() || !max_time.is_finite() {
            return Err("non-finite Niagara curve time".into());
        }
        let inverse_range = if samples.len() == channels {
            0.
        } else {
            p["LUTInvTimeRange"]
                .as_f64()
                .map(|n| n as f32)
                .filter(|n| n.is_finite() && *n > 0.)
                .or_else(|| (max_time > min_time).then(|| (max_time - min_time).recip()))
                .ok_or("invalid Niagara curve time range")?
        };
        Ok(Self {
            channels,
            samples,
            min_time,
            inverse_range,
        })
    }
    pub fn sample(&self, seconds: f32) -> Result<Vec<u32>, String> {
        // NaN/inf inputs clamp like the reference LUT lookup's saturate.
        let seconds = if seconds.is_nan() {
            self.min_time
        } else {
            seconds.clamp(-1e30, 1e30)
        };
        let last = self.samples.len() / self.channels - 1;
        let frame =
            ((seconds - self.min_time) * self.inverse_range * last as f32).clamp(0., last as f32);
        let a = frame.floor() as usize;
        let b = (a + 1).min(last);
        let t = frame - a as f32;
        Ok((0..self.channels)
            .map(|c| {
                let x = self.samples[a * self.channels + c];
                let y = self.samples[b * self.channels + c];
                (x + (y - x) * t).to_bits()
            })
            .collect())
    }
}
pub struct CpuContext {
    /// Original byte offsets address the concatenation in UE constant-table
    /// order. Use engine/system/owner/emitter/external/literal buffers exactly
    /// as declared by the imported script; arbitrary padding is not permitted.
    pub constants: Vec<u8>,
    pub datasets: BTreeMap<u16, Dataset>,
    pub curves: BTreeMap<String, Curve>,
    random: BTreeMap<u32, RandomStream>,
    seed: u64,
    lane: u32,
}
impl CpuContext {
    pub fn new(seed: u64) -> Self {
        Self {
            constants: Vec::new(),
            datasets: BTreeMap::new(),
            curves: BTreeMap::new(),
            random: BTreeMap::new(),
            seed,
            lane: 0,
        }
    }
    pub fn set_constant_buffers(&mut self, buffers: &[&[u8]]) -> Result<(), String> {
        let size = buffers.iter().try_fold(0usize, |a, b| {
            a.checked_add(b.len())
                .ok_or("Niagara constant table overflow")
        })?;
        if size > 256 * 1024 || buffers.iter().any(|b| b.len() % 4 != 0) {
            return Err("invalid Niagara constant table size/alignment".into());
        }
        self.constants.clear();
        self.constants.reserve(size);
        for b in buffers {
            self.constants.extend_from_slice(b);
        }
        Ok(())
    }
    pub fn execute(&mut self, kernel: &Kernel, instances: u32) -> Result<(), String> {
        if instances as usize > MAX_INSTANCES {
            return Err("Niagara instance budget exceeded".into());
        }
        for instance in 0..instances {
            self.lane = instance;
            kernel.execute(instance, self)?;
        }
        Ok(())
    }
}
impl Host for CpuContext {
    fn random_word(&mut self) -> Result<u32, String> {
        let seed = self.seed ^ ((self.lane as u64).wrapping_mul(0x9e3779b97f4a7c15));
        Ok(self
            .random
            .entry(self.lane)
            .or_insert_with(|| RandomStream::new(seed))
            .next_word())
    }
    fn constant(&self, offset: usize) -> Result<u32, String> {
        let end = offset
            .checked_add(4)
            .ok_or("Niagara constant offset overflow")?;
        let b = self
            .constants
            .get(offset..end)
            .ok_or_else(|| format!("unbound Niagara constant at byte {offset}"))?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn input(&self, d: u16, v: u16, i: u32) -> Result<u32, String> {
        self.input_typed(Numeric::Float, d, v, i)
    }
    fn output(&mut self, d: u16, v: u16, i: u32, w: u32) -> Result<(), String> {
        self.output_typed(Numeric::Float, d, v, i, w)
    }
    fn input_typed(&self, kind: Numeric, d: u16, v: u16, i: u32) -> Result<u32, String> {
        self.datasets
            .get(&d)
            .and_then(|d| d.input.get(&(kind, v)))
            .and_then(|c| c.get(i as usize))
            .copied()
            .ok_or_else(|| format!("unbound Niagara {kind:?} dataset {d} column {v} instance {i}"))
    }
    fn output_typed(
        &mut self,
        kind: Numeric,
        d: u16,
        v: u16,
        i: u32,
        w: u32,
    ) -> Result<(), String> {
        let data = self
            .datasets
            .get_mut(&d)
            .ok_or("unbound Niagara output dataset")?;
        if i as usize >= data.output_instances || v as usize >= MAX_COLUMNS {
            return Err("Niagara output outside allocated dataset".into());
        }
        if !data.output.contains_key(&(kind, v)) && data.output.len() >= MAX_COLUMNS {
            return Err("Niagara dataset column budget exceeded".into());
        }
        let values = data.output.entry((kind, v)).or_default();
        values.resize(data.output_instances, 0);
        values[i as usize] = w;
        Ok(())
    }
    fn acquire_index(&mut self, d: u16, valid: bool) -> Result<u32, String> {
        let data = self
            .datasets
            .get_mut(&d)
            .ok_or("unbound Niagara index dataset")?;
        if !valid {
            return Ok(u32::MAX);
        }
        if data.output_instances >= MAX_INSTANCES {
            return Err("Niagara particle budget exceeded".into());
        }
        let index = data.output_instances as u32;
        data.output_instances += 1;
        Ok(index)
    }
    fn acquire_id(&mut self, d: u16) -> Result<(u32, u32), String> {
        let data = self
            .datasets
            .get_mut(&d)
            .ok_or("unbound Niagara persistent ID dataset")?;
        let id = if let Some(id) = data.free_ids.pop() {
            id
        } else {
            if data.next_id as usize >= MAX_INSTANCES {
                return Err("Niagara persistent ID budget exceeded".into());
            }
            let id = data.next_id;
            data.next_id += 1;
            id
        };
        data.ids.insert(id, u32::MAX);
        Ok((id, data.acquire_tag))
    }
    fn update_id(&mut self, d: u16, id: u32, index: u32) -> Result<(), String> {
        let data = self
            .datasets
            .get_mut(&d)
            .ok_or("unbound Niagara persistent ID dataset")?;
        if !data.ids.contains_key(&id) {
            return Err("Niagara persistent ID is not allocated".into());
        }
        if index == u32::MAX {
            data.ids.remove(&id);
            data.free_ids.push(id);
        } else {
            if index as usize >= data.output_instances {
                return Err("Niagara ID maps outside output dataset".into());
            }
            data.ids.insert(id, index);
        }
        Ok(())
    }
    fn external(&mut self, f: &External, inputs: &[Option<u32>]) -> Result<Vec<u32>, String> {
        if matches!(
            f.name.as_str(),
            "SampleCurve" | "SampleVector2DCurve" | "SampleVectorCurve" | "SampleColorCurve"
        ) {
            let curve = self
                .curves
                .get(&f.owner)
                .ok_or("Niagara curve binding not found")?;
            // Cooked curve calls take only the sample time; there is no
            // per-instance user pointer for curve data interfaces.
            if inputs.is_empty() || f.outputs != curve.channels {
                return Err("Niagara curve signature mismatch".into());
            }
            return curve.sample(f32::from_bits(
                inputs[inputs.len() - 1].ok_or("Niagara curve time input not bound")?,
            ));
        }
        Err("Niagara data interface has no native binding".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn cooked_curve_luts_preserve_ranges_channels_and_constant_luts() {
        let curve=Curve::from_export(&json!({"Type":"NiagaraDataInterfaceVectorCurve","Properties":{"ShaderLUT":[1.,2.,3.,3.,6.,9.],"LUTMinTime":2.,"LUTMaxTime":4.,"LUTInvTimeRange":0.5}})).unwrap();
        assert_eq!(
            curve
                .sample(3.)
                .unwrap()
                .into_iter()
                .map(f32::from_bits)
                .collect::<Vec<_>>(),
            [2., 4., 6.]
        );
        assert_eq!(
            curve
                .sample(-1.)
                .unwrap()
                .into_iter()
                .map(f32::from_bits)
                .collect::<Vec<_>>(),
            [1., 2., 3.]
        );
        let curve=Curve::from_export(&json!({"Type":"NiagaraDataInterfaceCurve","Properties":{"ShaderLUT":[0.7],"LUTMinTime":0.,"LUTMaxTime":0.,"LUTInvTimeRange":"Infinity"}})).unwrap();
        assert_eq!(f32::from_bits(curve.sample(42.).unwrap()[0]), 0.7);
    }
    #[test]
    fn typed_datasets_half_conversion_and_persistent_id_reuse_are_independent() {
        // exec_index, float input, int input, half input; output all three.
        let bytes = [
            88, 0, 0, 77, 0, 0, 0, 0, 1, 0, 78, 0, 0, 0, 0, 2, 0, 79, 0, 0, 0, 0, 3, 0, 83, 0, 0,
            0, 0, 0, 1, 0, 0, 0, 84, 0, 0, 0, 0, 0, 2, 0, 0, 0, 85, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0,
        ];
        let kernel = Kernel::decode(&bytes, 4, vec![]).unwrap();
        let mut c = CpuContext::new(1);
        let mut d = Dataset::default();
        d.input.insert((Numeric::Float, 0), vec![2.5f32.to_bits()]);
        d.input.insert((Numeric::Int, 0), vec![99]);
        d.input.insert(
            (Numeric::Half, 0),
            vec![half::f16::from_f32(0.75).to_bits() as u32],
        );
        d.output_instances = 1;
        c.datasets.insert(0, d);
        c.execute(&kernel, 1).unwrap();
        let d = &c.datasets[&0];
        assert_eq!(d.output[&(Numeric::Float, 0)][0], 2.5f32.to_bits());
        assert_eq!(d.output[&(Numeric::Int, 0)][0], 99);
        assert_eq!(
            d.output[&(Numeric::Half, 0)][0],
            half::f16::from_f32(0.75).to_bits() as u32
        );
        let (id, tag) = c.acquire_id(0).unwrap();
        c.update_id(0, id, 0).unwrap();
        assert_eq!(c.datasets[&0].id_instance(id), Some(0));
        c.update_id(0, id, u32::MAX).unwrap();
        assert!(c.update_id(0, id, u32::MAX).is_err());
        c.datasets.get_mut(&0).unwrap().begin_pass();
        let (again, new_tag) = c.acquire_id(0).unwrap();
        assert_eq!(again, id);
        assert_ne!(new_tag, tag);
    }
    #[test]
    fn persistent_id_bytecode_uses_dataset_and_two_register_operands() {
        // acquire_id dataset0->registers0/1, exec_index->2, update_id 0,0,2.
        let kernel = Kernel::decode(
            &[94, 0, 0, 0, 0, 1, 0, 88, 2, 0, 93, 0, 0, 0, 0, 2, 0, 0],
            3,
            vec![],
        )
        .unwrap();
        let mut c = CpuContext::new(1);
        let mut d = Dataset::default();
        d.output_instances = 2;
        c.datasets.insert(0, d);
        c.execute(&kernel, 2).unwrap();
        assert_eq!(c.datasets[&0].id_instance(0), Some(0));
        assert_eq!(c.datasets[&0].id_instance(1), Some(1));
    }
}
