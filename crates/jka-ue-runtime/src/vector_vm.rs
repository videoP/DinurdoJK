//! Bounded scalar execution of Unreal's original Niagara VectorVM bytecode.
//! Opcode/operand ABI verified against UE 5.8 VectorVMOptimizer.cpp. Engine
//! datasets, constant tables and data-interface functions are supplied by Host;
//! no missing binding is fabricated. GPU/optimized bytecode is a different ABI.
#[derive(Debug, Clone)]
pub struct External {
    pub owner: String,
    pub name: String,
    pub inputs: usize,
    pub outputs: usize,
}
#[derive(Debug, Clone, Copy)]
enum Operand {
    Register(usize),
    Constant(usize),
    Unused,
}
#[derive(Debug)]
enum Instruction {
    Math(u8, Vec<Operand>, usize),
    Index(usize),
    Input {
        kind: Numeric,
        dataset: u16,
        variable: u16,
        dest: usize,
        advance: bool,
    },
    Output {
        kind: Numeric,
        dataset: u16,
        index: usize,
        value: Operand,
        variable: u16,
    },
    Acquire {
        dataset: u16,
        valid: Operand,
        dest: usize,
    },
    External(usize, Vec<Operand>, Vec<Option<usize>>),
    AcquireId(u16, usize, usize),
    UpdateId(u16, usize, usize),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Numeric {
    Float,
    Int,
    Half,
}
#[derive(Debug)]
pub struct Kernel {
    instructions: Vec<Instruction>,
    pub external: Vec<External>,
    registers: usize,
}
/// Host values are raw 32-bit words, preserving int/bool/float reinterpretation.
pub trait Host {
    /// Persistent per-system/per-lane stream; never reset every update tick.
    fn random_word(&mut self) -> Result<u32, String> {
        Err("Niagara random stream is not bound".into())
    }
    fn constant(&self, byte_offset: usize) -> Result<u32, String>;
    fn input(&self, dataset: u16, variable: u16, instance: u32) -> Result<u32, String>;
    fn output(&mut self, dataset: u16, variable: u16, index: u32, value: u32)
        -> Result<(), String>;
    fn acquire_index(&mut self, dataset: u16, valid: bool) -> Result<u32, String>;
    fn external(&mut self, function: &External, inputs: &[Option<u32>])
        -> Result<Vec<u32>, String>;
    /// Float and int register zero are separate dataset columns. Hosts with
    /// mixed datasets override these methods; half values are raw u16 bits.
    fn input_typed(
        &self,
        _kind: Numeric,
        dataset: u16,
        variable: u16,
        instance: u32,
    ) -> Result<u32, String> {
        self.input(dataset, variable, instance)
    }
    fn output_typed(
        &mut self,
        _kind: Numeric,
        dataset: u16,
        variable: u16,
        index: u32,
        value: u32,
    ) -> Result<(), String> {
        self.output(dataset, variable, index, value)
    }
    fn acquire_id(&mut self, _dataset: u16) -> Result<(u32, u32), String> {
        Err("Niagara persistent ID allocator is not bound".into())
    }
    /// Output index UINT_MAX frees the ID; otherwise update its instance map.
    fn update_id(&mut self, _dataset: u16, _id: u32, _output_index: u32) -> Result<(), String> {
        Err("Niagara persistent ID table is not bound".into())
    }
}
/// Independent lane state for the reference runtime's XORWOW stream. The seed
/// is supplied by the owning emitter, so deterministic systems can be replayed.
#[derive(Clone, Debug)]
pub struct RandomStream {
    state: [u32; 5],
    counter: u32,
}
impl RandomStream {
    pub fn new(seed: u64) -> Self {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            (z ^ (z >> 31)) as u32
        };
        let mut state = std::array::from_fn(|_| next());
        if state.iter().all(|n| *n == 0) {
            state[0] = 1;
        }
        Self {
            state,
            counter: next(),
        }
    }
    pub fn next_word(&mut self) -> u32 {
        let mut t = self.state[4];
        let s = self.state[0];
        self.state.copy_within(0..4, 1);
        t ^= t >> 2;
        t ^= t << 1;
        t ^= s ^ (s ^ (s << 4));
        self.state[0] = t;
        self.counter = self.counter.wrapping_add(362437);
        t.wrapping_add(self.counter)
    }
}
impl Kernel {
    pub fn decode(bytes: &[u8], registers: usize, external: Vec<External>) -> Result<Self, String> {
        if bytes.len() > 4 * 1024 * 1024
            || registers > 4096
            || registers == 0
            || external.len() > 4096
        {
            return Err("Niagara kernel exceeds limits".into());
        }
        let mut at = 0;
        let mut instructions = Vec::new();
        fn byte(bytes: &[u8], at: &mut usize) -> Result<u8, String> {
            let n = *bytes.get(*at).ok_or("truncated VectorVM instruction")?;
            *at += 1;
            Ok(n)
        }
        fn word(bytes: &[u8], at: &mut usize) -> Result<u16, String> {
            let a = byte(bytes, at)?;
            let b = byte(bytes, at)?;
            Ok(u16::from_le_bytes([a, b]))
        }
        let register = |n: u16| -> Result<usize, String> {
            if n as usize >= registers {
                return Err(format!("VectorVM register {n} outside 0..{registers}"));
            }
            Ok(n as usize)
        };
        let operand = |n: u16, constant: bool| -> Result<Operand, String> {
            if constant {
                if n % 4 != 0 {
                    return Err("unaligned Niagara constant".into());
                }
                Ok(Operand::Constant(n as usize))
            } else {
                Ok(Operand::Register(register(n)?))
            }
        };
        let mut done = false;
        while at < bytes.len() {
            if instructions.len() > 100_000 {
                return Err("Niagara instruction budget exceeded".into());
            }
            let opcode = byte(bytes, &mut at)?;
            match opcode {
                0 => {
                    done = true;
                    break;
                }
                91 => {
                    word(bytes, &mut at)?;
                }
                92 => {}
                88 => instructions.push(Instruction::Index(register(word(bytes, &mut at)?)?)),
                77..=82 => {
                    let dataset = word(bytes, &mut at)?;
                    let variable = word(bytes, &mut at)?;
                    let dest = register(word(bytes, &mut at)?)?;
                    instructions.push(Instruction::Input {
                        kind: match opcode {
                            77 | 80 => Numeric::Float,
                            78 | 81 => Numeric::Int,
                            _ => Numeric::Half,
                        },
                        dataset,
                        variable,
                        dest,
                        advance: opcode < 80,
                    });
                }
                83..=85 => {
                    let constant = byte(bytes, &mut at)? & 1 != 0;
                    let dataset = word(bytes, &mut at)?;
                    let index = register(word(bytes, &mut at)?)?;
                    let value = operand(word(bytes, &mut at)?, constant)?;
                    let variable = word(bytes, &mut at)?;
                    instructions.push(Instruction::Output {
                        kind: match opcode {
                            83 => Numeric::Float,
                            84 => Numeric::Int,
                            _ => Numeric::Half,
                        },
                        dataset,
                        index,
                        value,
                        variable,
                    });
                }
                86 => {
                    let constant = byte(bytes, &mut at)? & 1 != 0;
                    let dataset = word(bytes, &mut at)?;
                    let valid = operand(word(bytes, &mut at)?, constant)?;
                    let dest = register(word(bytes, &mut at)?)?;
                    instructions.push(Instruction::Acquire {
                        dataset,
                        valid,
                        dest,
                    });
                }
                87 => {
                    let index = word(bytes, &mut at)? as usize;
                    let signature = external
                        .get(index)
                        .ok_or("Niagara external function outside signature table")?;
                    if signature.inputs > 128 || signature.outputs > 128 {
                        return Err("Niagara function signature exceeds limit".into());
                    }
                    let mut inputs = Vec::new();
                    for _ in 0..signature.inputs {
                        let n = word(bytes, &mut at)?;
                        inputs.push(if n == 0xffff {
                            Operand::Unused
                        } else {
                            operand(n & 0x7fff, n & 0x8000 == 0)?
                        });
                    }
                    let mut outputs = Vec::new();
                    for _ in 0..signature.outputs {
                        let n = word(bytes, &mut at)?;
                        outputs.push(if n == 0xffff {
                            None
                        } else {
                            Some(register(n)?)
                        });
                    }
                    instructions.push(Instruction::External(index, inputs, outputs));
                }
                93 | 94 => {
                    let dataset = word(bytes, &mut at)?;
                    let id = register(word(bytes, &mut at)?)?;
                    let index_or_tag = register(word(bytes, &mut at)?)?;
                    instructions.push(if opcode == 93 {
                        Instruction::UpdateId(dataset, id, index_or_tag)
                    } else {
                        Instruction::AcquireId(dataset, id, index_or_tag)
                    });
                }
                // Optimized merged instructions have a different ABI.
                1..=76 | 89 | 90 | 95..=97 => {
                    let count = match opcode {
                        5 | 6 | 28 | 43 | 48 | 90 => 3,
                        1..=4
                        | 22
                        | 25
                        | 29..=31
                        | 34
                        | 37..=42
                        | 44..=47
                        | 49
                        | 50
                        | 55..=63
                        | 65..=69
                        | 89 => 2,
                        _ => 1,
                    };
                    let flags = byte(bytes, &mut at)?;
                    if flags >> count != 0 {
                        return Err("invalid VectorVM operand flags".into());
                    }
                    let mut inputs = Vec::new();
                    for i in 0..count {
                        inputs.push(operand(word(bytes, &mut at)?, flags & (1 << i) != 0)?);
                    }
                    let dest = register(word(bytes, &mut at)?)?;
                    instructions.push(Instruction::Math(opcode, inputs, dest));
                }
                _ => {
                    return Err(format!(
                        "unsupported Niagara opcode {opcode} at byte {}",
                        at - 1
                    ));
                }
            }
        }
        if !done {
            return Err("Niagara kernel missing Done instruction".into());
        }
        if bytes[at..].iter().any(|n| *n != 0) {
            return Err("Niagara instructions after Done".into());
        }
        Ok(Self {
            instructions,
            registers,
            external,
        })
    }
    pub fn execute(&self, instance: u32, host: &mut impl Host) -> Result<(), String> {
        let mut registers = vec![None; self.registers];
        let value = |operand: Operand,
                     regs: &[Option<u32>],
                     host: &dyn Host|
         -> Result<Option<u32>, String> {
            match operand {
                Operand::Unused => Ok(None),
                Operand::Constant(n) => host.constant(n).map(Some),
                Operand::Register(n) => regs[n]
                    .ok_or("read of uninitialized Niagara register".into())
                    .map(Some),
            }
        };
        for instruction in &self.instructions {
            match instruction {
                Instruction::Math(op, inputs, dest) => {
                    let mut words = [0; 3];
                    for (i, &input) in inputs.iter().enumerate() {
                        words[i] = value(input, &registers, host)?.ok_or("unused math operand")?;
                    }
                    let result = if matches!(*op, 35 | 54) {
                        let fraction = f32::from_bits(0x3f800000 | (host.random_word()? >> 9)) - 1.;
                        if *op == 35 {
                            (fraction * f32::from_bits(words[0])).to_bits()
                        } else {
                            (fraction * (words[0] as i32) as f32).trunc() as i32 as u32
                        }
                    } else {
                        math(*op, words)?
                    };
                    registers[*dest] = Some(result);
                }
                Instruction::Index(dest) => registers[*dest] = Some(instance),
                Instruction::Input {
                    kind,
                    dataset,
                    variable,
                    dest,
                    advance,
                } => {
                    let word = host.input_typed(
                        *kind,
                        *dataset,
                        *variable,
                        if *advance { instance } else { 0 },
                    )?;
                    registers[*dest] = Some(if *kind == Numeric::Half {
                        half::f16::from_bits(word as u16).to_f32().to_bits()
                    } else {
                        word
                    });
                }
                Instruction::Output {
                    kind,
                    dataset,
                    index,
                    value: input,
                    variable,
                } => {
                    let index = registers[*index].ok_or("uninitialized Niagara output index")?;
                    if index != u32::MAX {
                        let word = value(*input, &registers, host)?.ok_or("unused output value")?;
                        let word = if *kind == Numeric::Half {
                            half::f16::from_f32(f32::from_bits(word)).to_bits() as u32
                        } else {
                            word
                        };
                        host.output_typed(*kind, *dataset, *variable, index, word)?;
                    }
                }
                Instruction::Acquire {
                    dataset,
                    valid,
                    dest,
                } => {
                    let valid =
                        value(*valid, &registers, host)?.ok_or("unused acquire predicate")? != 0;
                    registers[*dest] = Some(host.acquire_index(*dataset, valid)?);
                }
                Instruction::External(index, inputs, outputs) => {
                    let args = inputs
                        .iter()
                        .map(|input| value(*input, &registers, host))
                        .collect::<Result<Vec<_>, _>>()?;
                    let signature = &self.external[*index];
                    let result = host
                        .external(signature, &args)
                        .map_err(|e| format!("{}.{}: {e}", signature.owner, signature.name))?;
                    if result.len() != outputs.len() {
                        return Err("Niagara external output count mismatch".into());
                    }
                    for (dest, word) in outputs.iter().zip(result) {
                        if let Some(dest) = dest {
                            registers[*dest] = Some(word);
                        }
                    }
                }
                Instruction::AcquireId(dataset, id, tag) => {
                    let (a, b) = host.acquire_id(*dataset)?;
                    registers[*id] = Some(a);
                    registers[*tag] = Some(b);
                }
                Instruction::UpdateId(dataset, id, index) => host.update_id(
                    *dataset,
                    registers[*id].ok_or("uninitialized Niagara persistent ID")?,
                    registers[*index].ok_or("uninitialized Niagara persistent ID output index")?,
                )?,
            }
        }
        Ok(())
    }
}
fn math(op: u8, words: [u32; 3]) -> Result<u32, String> {
    let [a, b, c] = words.map(f32::from_bits);
    let [i, j, k] = words.map(|n| n as i32);
    let boolean = |n: bool| if n { u32::MAX } else { 0 };
    let float = match op {
        1 => a + b,
        2 => a - b,
        3 => a * b,
        4 => a / b,
        5 => a * b + c,
        6 => a + (b - a) * c,
        7 => a.recip(),
        8 => a.sqrt().recip(),
        9 => a.sqrt(),
        10 => -a,
        11 => a.abs(),
        12 => a.exp(),
        13 => a.exp2(),
        14 => a.ln(),
        15 => a.log2(),
        16 => a.sin(),
        17 => a.cos(),
        18 => a.tan(),
        19 => a.asin(),
        20 => a.acos(),
        21 => a.atan(),
        22 => a.atan2(b),
        23 => a.ceil(),
        24 => a.floor(),
        25 => a % b,
        26 => a - a.floor(),
        27 => a.trunc(),
        28 => a.max(b).min(c),
        29 => a.min(b),
        30 => a.max(b),
        31 => a.powf(b),
        32 => a.round_ties_even(),
        33 => {
            if a == 0. {
                0.
            } else {
                a.signum()
            }
        }
        34 => {
            if a >= b {
                1.
            } else {
                0.
            }
        }
        // The referenced UE VectorVM runtime returns zero for the legacy
        // noise instructions. Niagara's actual procedural noise uses DIs.
        36 | 89 | 90 => 0.,
        95 => half::f16::from_bits(words[0] as u16).to_f32(),
        37 => return Ok(boolean(a < b)),
        38 => return Ok(boolean(a <= b)),
        39 => return Ok(boolean(a > b)),
        40 => return Ok(boolean(a >= b)),
        41 => return Ok(boolean(a == b)),
        42 => return Ok(boolean(a != b)),
        43 => return Ok((words[0] & words[1]) | (!words[0] & words[2])),
        44 => return Ok(i.wrapping_add(j) as u32),
        45 => return Ok(i.wrapping_sub(j) as u32),
        46 => return Ok(i.wrapping_mul(j) as u32),
        47 => {
            return Ok(if j == 0 {
                0
            } else if i == i32::MIN && j == -1 {
                i32::MAX as u32
            } else {
                (i / j) as u32
            });
        }
        48 => return Ok(i.max(j).min(k) as u32),
        49 => return Ok(i.min(j) as u32),
        50 => return Ok(i.max(j) as u32),
        51 => return Ok(i.wrapping_abs() as u32),
        52 => return Ok(i.wrapping_neg() as u32),
        53 => return Ok(i.signum() as u32),
        55 => return Ok(boolean(i < j)),
        56 => return Ok(boolean(i <= j)),
        57 => return Ok(boolean(i > j)),
        58 => return Ok(boolean(i >= j)),
        59 => return Ok(boolean(i == j)),
        60 => return Ok(boolean(i != j)),
        61 | 67 => return Ok(words[0] & words[1]),
        62 | 68 => return Ok(words[0] | words[1]),
        63 | 69 => return Ok(words[0] ^ words[1]),
        64 | 70 => return Ok(!words[0]),
        65 => return Ok(words[0].wrapping_shl(words[1])),
        66 => return Ok(i.wrapping_shr(words[1]) as u32),
        71 => return Ok(a as i32 as u32),
        72 => i as f32,
        73 => return Ok(boolean(a != 0.)),
        74 => {
            if i != 0 {
                1.
            } else {
                0.
            }
        }
        75 => return Ok(boolean(i != 0)),
        76 => return Ok(if i != 0 { 1 } else { 0 }),
        96 | 97 => return Ok(words[0]),
        _ => return Err(format!("unsupported Niagara math opcode {op}")),
    };
    Ok(float.to_bits())
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Data {
        values: Vec<f32>,
    }
    impl Host for Data {
        fn constant(&self, offset: usize) -> Result<u32, String> {
            if offset == 0 {
                Ok(2f32.to_bits())
            } else {
                Err("unbound constant".into())
            }
        }
        fn input(&self, _: u16, _: u16, _: u32) -> Result<u32, String> {
            Err("unbound dataset".into())
        }
        fn output(&mut self, _: u16, _: u16, index: u32, value: u32) -> Result<(), String> {
            if index as usize != self.values.len() {
                return Err("unexpected output index".into());
            }
            self.values.push(f32::from_bits(value));
            Ok(())
        }
        fn acquire_index(&mut self, _: u16, valid: bool) -> Result<u32, String> {
            Ok(if valid {
                self.values.len() as u32
            } else {
                u32::MAX
            })
        }
        fn external(&mut self, _: &External, _: &[Option<u32>]) -> Result<Vec<u32>, String> {
            Err("missing native binding".into())
        }
    }
    #[test]
    fn original_niagara_bytecode_executes_constant_math_and_dataset_output() {
        let bytes = [
            88, 0, 0, 72, 0, 0, 0, 1, 0, 1, 2, 1, 0, 0, 0, 2, 0, 83, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0,
        ];
        let kernel = Kernel::decode(&bytes, 3, vec![]).unwrap();
        let mut data = Data { values: vec![] };
        kernel.execute(0, &mut data).unwrap();
        kernel.execute(1, &mut data).unwrap();
        assert_eq!(data.values, vec![2., 3.]);
    }
    #[test]
    fn corrupt_bytecode_and_missing_engine_bindings_never_succeed() {
        assert!(Kernel::decode(&[88, 255, 255, 0], 3, vec![]).is_err());
        assert!(Kernel::decode(&[1, 0], 3, vec![]).is_err());
        let k = Kernel::decode(
            &[87, 0, 0, 0],
            1,
            vec![External {
                owner: "MyGame".into(),
                name: "Missing".into(),
                inputs: 0,
                outputs: 0,
            }],
        )
        .unwrap();
        let error = k.execute(0, &mut Data { values: vec![] }).unwrap_err();
        assert!(error.contains("MyGame.Missing"));
    }
    #[test]
    fn random_state_is_replayable_and_survives_updates() {
        let mut a = RandomStream::new(123);
        let mut b = RandomStream::new(123);
        let first = a.next_word();
        assert_eq!(first, b.next_word());
        for _ in 0..1000 {
            assert_eq!(a.next_word(), b.next_word());
        }
        assert_ne!(first, a.next_word());
        assert_ne!(RandomStream::new(124).next_word(), first);
        // Check the transition independently of seed generation.
        let mut known = RandomStream {
            state: [1, 2, 3, 4, 5],
            counter: 7,
        };
        assert_eq!(known.next_word(), 362472);
        assert_eq!(known.state, [28, 1, 2, 3, 4]);
        assert_eq!(math(47, [1, 0, 0]).unwrap(), 0);
        assert_eq!(
            math(47, [i32::MIN as u32, (-1i32) as u32, 0]).unwrap(),
            i32::MAX as u32
        );
    }
}
