//! An interactive chip: set the inputs, evaluate or clock it, read every pin.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use n2t_core::builtins::BuiltinLibrary;
use n2t_core::hdl::definition::{ChipDefinition, InputSource, NetKind};
use n2t_core::hdl::interface::ChipOrigin;
use n2t_core::hdl::resolve::{HostSourceProvider, SourceLookup, SourceProvider};
use n2t_core::host::{SharedHost, path};
use n2t_core::source::{LineIndex, Span};
use n2t_simulator::{Circuit, HardwareMachine, InstanceView, Machine};
use serde_json::{Value, json};

use crate::{ErrorCode, ErrorReply, Failure, observed, text};

type ChipError = (ErrorCode, String);
type ChipReply = Result<Value, ChipError>;

/// Largest unsigned integer that JavaScript represents exactly as a `number` (2^53 - 1).
const MAX_SAFE_JSON_INT: u64 = (1u64 << 53) - 1;

pub struct ChipSession {
    host: SharedHost,
    file: String,
    name: String,
    machine: HardwareMachine,
    /// Source texts captured at build/reset time (`file_id -> text`) so
    /// `chip.diagram` hashes and indexes the elaborated source without
    /// re-reading the host or overlay.
    sources: HashMap<String, Arc<str>>,
    revision: u64,
}

impl ChipSession {
    /// `{"op":"chip.open","path":…}`.
    pub fn open(host: &SharedHost, request: &Value) -> Result<Self, ErrorReply> {
        let file = path::normalize(
            text(request, "path").map_err(|m| ErrorReply::coded(ErrorCode::Other, m))?,
        );
        let name = path::stem(&file).to_string();
        let (machine, sources) =
            load(host, &file, &name).map_err(|m| ErrorReply::coded(ErrorCode::BuildFailed, m))?;
        Ok(Self {
            host: host.clone(),
            file,
            name,
            machine,
            sources,
            revision: 1,
        })
    }

    pub fn call(&mut self, op: &str, request: &Value) -> Result<Value, ErrorReply> {
        self.call_inner(op, request).map_err(|(code, message)| {
            ErrorReply::coded(code, message).with_revision(self.revision)
        })
    }

    fn call_inner(&mut self, op: &str, request: &Value) -> ChipReply {
        self.check_revision(request)?;
        match op {
            "chip.state" => {}
            "chip.set" => {
                let pin = text(request, "pin").map_err(|m| (ErrorCode::InvalidInput, m))?;
                let value = &request["value"];
                self.set(pin, value)?;
                if request["eval"].as_bool().unwrap_or(false) {
                    self.circuit()?.eval();
                }
            }
            "chip.eval" => self.circuit()?.eval(),
            "chip.tick" => self
                .circuit()?
                .tick()
                .map_err(|e| (ErrorCode::Other, e.to_string()))?,
            "chip.tock" => self
                .circuit()?
                .tock()
                .map_err(|e| (ErrorCode::Other, e.to_string()))?,
            "chip.step" => {
                // A whole clock cycle, from whichever phase the clock is in.
                let circuit = self.circuit()?;
                if circuit.time().after_tick {
                    circuit
                        .tock()
                        .map_err(|e| (ErrorCode::Other, e.to_string()))?;
                } else {
                    circuit
                        .tick()
                        .map_err(|e| (ErrorCode::Other, e.to_string()))?;
                    circuit
                        .tock()
                        .map_err(|e| (ErrorCode::Other, e.to_string()))?;
                }
            }
            "chip.reset" => {
                // Reload from the source, so an edit takes effect, and keep
                // the inputs the student had set.
                let inputs: Vec<(String, u64)> = self
                    .machine
                    .observe()
                    .into_iter()
                    .filter(|o| o.role == n2t_simulator::Role::Input)
                    .map(|o| (o.name, o.bits))
                    .collect();
                let (mut machine, sources) = load(&self.host, &self.file, &self.name)
                    .map_err(|m| (ErrorCode::BuildFailed, m))?;
                if request["keepInputs"].as_bool().unwrap_or(true) {
                    let circuit = machine
                        .circuit_mut()
                        .ok_or_else(|| (ErrorCode::Other, "no chip is loaded".to_string()))?;
                    for (name, bits) in inputs {
                        circuit.set(&name, bits);
                    }
                    circuit.eval();
                }
                self.machine = machine;
                self.sources = sources;
                self.revision += 1;
            }
            "key" => {
                let code = crate::key_code(request).map_err(|m| (ErrorCode::Other, m))?;
                return Ok(json!({ "accepted": self.machine.set_key(code) }));
            }
            "chip.table" => return self.table(),
            "chip.diagram" => return self.diagram(request),
            "chip.values" => return self.values(request),
            other => {
                return Err((
                    ErrorCode::Other,
                    format!("`{other}` is not an operation of a chip"),
                ));
            }
        }
        Ok(self.state())
    }

    fn check_revision(&self, request: &Value) -> Result<(), ChipError> {
        match request.get("expectRevision") {
            None | Some(Value::Null) => Ok(()),
            Some(value) => {
                let expected = value.as_u64().ok_or_else(|| {
                    (
                        ErrorCode::Other,
                        "`expectRevision` must be a non-negative whole number".to_string(),
                    )
                })?;
                if expected != self.revision {
                    return Err((
                        ErrorCode::StaleRevision,
                        format!(
                            "stale revision: expected {expected}, current is {}",
                            self.revision
                        ),
                    ));
                }
                Ok(())
            }
        }
    }

    fn circuit(&mut self) -> Result<&mut Circuit, ChipError> {
        self.machine
            .circuit_mut()
            .ok_or_else(|| (ErrorCode::Other, "no chip is loaded".to_string()))
    }

    fn circuit_ref(&self) -> Result<&Circuit, ChipError> {
        self.machine
            .circuit()
            .ok_or_else(|| (ErrorCode::Other, "no chip is loaded".to_string()))
    }

    fn set(&mut self, pin: &str, value: &Value) -> Result<(), ChipError> {
        let name = self.name.clone();
        let circuit = self.circuit()?;
        let width = circuit
            .definition()
            .interface
            .input(pin)
            .map(|(_, info)| info.width)
            .ok_or_else(|| {
                (
                    ErrorCode::InvalidInput,
                    format!("`{name}` has no input pin named `{pin}`"),
                )
            })?;
        let bits = parse_value(value, width).map_err(|m| (ErrorCode::InvalidInput, m))?;
        circuit.set(pin, bits);
        Ok(())
    }

    /// The truth table of a small combinational chip: every combination of
    /// its inputs with the outputs it produces. The chip's current inputs
    /// are put back afterwards.
    fn table(&mut self) -> ChipReply {
        const MAX_INPUT_BITS: u32 = 6;
        let circuit = self.circuit()?;
        let definition = circuit.definition().clone();
        let inputs: Vec<(String, u32)> = definition
            .interface
            .inputs
            .iter()
            .map(|p| (p.name.clone(), p.width))
            .collect();
        let outputs: Vec<(String, u32)> = definition
            .interface
            .outputs
            .iter()
            .map(|p| (p.name.clone(), p.width))
            .collect();
        let bits: u32 = inputs.iter().map(|(_, w)| *w).sum();
        if definition.sequential {
            return Err((
                ErrorCode::Other,
                "a clocked chip has no truth table: its outputs depend on time".to_string(),
            ));
        }
        if bits > MAX_INPUT_BITS {
            return Err((
                ErrorCode::Other,
                format!(
                    "the chip has {bits} input bits; a truth table is shown for up to {MAX_INPUT_BITS}"
                ),
            ));
        }
        let saved: Vec<u64> = inputs
            .iter()
            .map(|(name, _)| circuit.get(name).unwrap_or(0))
            .collect();
        let mut rows = Vec::new();
        for combination in 0u64..(1 << bits) {
            // The first input is the most significant, as truth tables are
            // usually written.
            let mut shift = bits;
            let mut row_inputs = Vec::new();
            for (name, width) in &inputs {
                shift -= width;
                let value = (combination >> shift) & ((1 << width) - 1);
                circuit.set(name, value);
                row_inputs.push(value);
            }
            circuit.eval();
            let row_outputs: Vec<u64> = outputs
                .iter()
                .map(|(name, _)| circuit.get(name).unwrap_or(0))
                .collect();
            rows.push(json!({ "inputs": row_inputs, "outputs": row_outputs }));
        }
        for ((name, _), value) in inputs.iter().zip(saved) {
            circuit.set(name, value);
        }
        circuit.eval();
        let columns = |pins: &[(String, u32)]| -> Vec<Value> {
            pins.iter()
                .map(|(name, width)| json!({ "name": name, "width": width }))
                .collect()
        };
        Ok(json!({
            "inputs": columns(&inputs),
            "outputs": columns(&outputs),
            "rows": rows,
        }))
    }

    pub fn state(&self) -> Value {
        let (time, clocked) = match self.machine.circuit() {
            Some(circuit) => (circuit.time().to_string(), circuit.definition().sequential),
            None => (String::new(), false),
        };
        json!({
            "chip": self.name,
            "revision": self.revision,
            "clocked": clocked,
            "time": time,
            "pins": observed(&self.machine.observe()),
        })
    }

    /// `{"op":"chip.diagram","path":[…]}`: structure and live values of the
    /// composite chip instance at `path` (`[]` = root).
    fn diagram(&self, request: &Value) -> ChipReply {
        let instance_path = parse_path(request)?;
        let circuit = self.circuit_ref()?;
        let (breadcrumb, view) = resolve_composite(circuit, &instance_path)?;
        let def = view.definition();
        let Some(composite) = def.as_composite() else {
            return Err((
                ErrorCode::InvalidPath,
                format!(
                    "chip `{}` is built-in and has no internal diagram",
                    def.name
                ),
            ));
        };
        let (file, source_hash, lines) = self.source_for_chip(def);
        let nets: Vec<Value> = composite
            .nets
            .iter()
            .map(|net| {
                let kind = match net.kind {
                    NetKind::Input(_) => "input",
                    NetKind::Output(_) => "output",
                    NetKind::Internal => "internal",
                };
                json!({
                    "name": net.name,
                    "width": net.width,
                    "kind": kind,
                })
            })
            .collect();
        let parts: Vec<Value> = composite
            .parts
            .iter()
            .map(|part| {
                let line = lines
                    .as_ref()
                    .map_or(0, |idx| idx.position(part.span.start).line);
                let end_line = lines
                    .as_ref()
                    .map_or(0, |idx| idx.position(part.span.end).line);
                let mut conns = Vec::with_capacity(part.inputs.len() + part.outputs.len());
                for conn in &part.inputs {
                    let pin_bits = [conn.pin_bits.start, conn.pin_bits.width];
                    match conn.source {
                        InputSource::Net { net, bits } => conns.push(json!({
                            "dir": "in",
                            "pin": conn.pin,
                            "pinBits": pin_bits,
                            "net": net,
                            "netBits": [bits.start, bits.width],
                        })),
                        InputSource::Constant(value) => conns.push(json!({
                            "dir": "in",
                            "pin": conn.pin,
                            "pinBits": pin_bits,
                            "constant": value,
                        })),
                    }
                }
                for conn in &part.outputs {
                    conns.push(json!({
                        "dir": "out",
                        "pin": conn.pin,
                        "pinBits": [conn.pin_bits.start, conn.pin_bits.width],
                        "net": conn.net,
                        "netBits": [conn.net_bits.start, conn.net_bits.width],
                    }));
                }
                json!({
                    "chip": part.chip.name,
                    "drill": part.chip.as_composite().is_some(),
                    "builtin": part.chip.as_builtin().is_some(),
                    "sequential": part.chip.sequential,
                    "span": {
                        "start": part.span.start,
                        "end": part.span.end,
                        "line": line,
                        "endLine": end_line,
                    },
                    "inputs": pin_list(&part.chip.interface.inputs),
                    "outputs": pin_list(&part.chip.interface.outputs),
                    "deps": &part.chip.combinational_dependencies,
                    "conns": conns,
                })
            })
            .collect();
        let values = values_json(circuit, view, self.revision, &instance_path)?;
        Ok(json!({
            "revision": self.revision,
            "path": instance_path,
            "chip": def.name,
            "file": file,
            "sourceHash": source_hash,
            "breadcrumb": breadcrumb,
            "inputs": pin_list(&def.interface.inputs),
            "outputs": pin_list(&def.interface.outputs),
            "nets": nets,
            "parts": parts,
            "values": values,
        }))
    }

    /// `{"op":"chip.values","path":[…]}`: current net and part-pin values for
    /// the composite chip instance at `path`.
    fn values(&self, request: &Value) -> ChipReply {
        let instance_path = parse_path(request)?;
        let circuit = self.circuit_ref()?;
        let (_, view) = resolve_composite(circuit, &instance_path)?;
        values_json(circuit, view, self.revision, &instance_path)
    }

    fn source_for_chip(
        &self,
        def: &ChipDefinition,
    ) -> (Option<String>, Option<String>, Option<LineIndex>) {
        match &def.interface.origin {
            ChipOrigin::Builtin => (None, None, None),
            ChipOrigin::Source { id } => {
                let text = self.sources.get(id);
                let hash = text.map(|t| source_hash(t));
                let lines = text.map(|t| LineIndex::new(t));
                (Some(id.clone()), hash, lines)
            }
        }
    }
}

/// 64-bit FNV-1a hash of `text`'s UTF-8 bytes, formatted as 16 lowercase
/// hexadecimal digits (left-padded with `'0'`, no `0x` prefix).
///
/// Algorithm (for TypeScript / editor parity over `document.getText()`):
/// 1. Encode the exact text as UTF-8 bytes (no line-ending normalisation).
/// 2. Start with `hash = 0xcbf29ce484222325` (FNV-1a 64-bit offset basis).
/// 3. For each byte `b`:
///    `hash = ((hash ^ b) * 0x00000100000001b3) mod 2^64`.
/// 4. Format `hash` as 16 lowercase hexadecimal characters, zero-padded on the left.
///
/// Test vector:
/// - `source_hash("CHIP X {}\n") == "f3235dc7995af525"`
fn source_hash(text: &str) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in text.as_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

fn pin_list(pins: &[n2t_core::hdl::interface::PinInfo]) -> Vec<Value> {
    pins.iter()
        .map(|pin| json!({ "name": pin.name, "width": pin.width }))
        .collect()
}

fn encode_bits(bits: u64) -> Value {
    if bits <= MAX_SAFE_JSON_INT {
        json!(bits)
    } else {
        json!(bits.to_string())
    }
}

fn parse_path(request: &Value) -> Result<Vec<usize>, ChipError> {
    match request.get("path") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| {
                        (
                            ErrorCode::InvalidPath,
                            "each entry of `path` must be a non-negative whole number".to_string(),
                        )
                    })
            })
            .collect(),
        Some(_) => Err((
            ErrorCode::InvalidPath,
            "`path` must be a list of part indices".to_string(),
        )),
    }
}

fn resolve_composite<'a>(
    circuit: &'a Circuit,
    instance_path: &[usize],
) -> Result<(Vec<Value>, InstanceView<'a>), ChipError> {
    let mut view = circuit.instance(&[]).ok_or_else(|| {
        (
            ErrorCode::Other,
            "circuit probes are not enabled for this session".to_string(),
        )
    })?;
    let mut breadcrumb = Vec::with_capacity(instance_path.len() + 1);
    breadcrumb.push(json!({
        "chip": view.definition().name,
        "path": [],
    }));
    for (depth, &part_index) in instance_path.iter().enumerate() {
        let Some(composite) = view.definition().as_composite() else {
            return Err((
                ErrorCode::InvalidPath,
                format!(
                    "chip `{}` is built-in and has no parts to inspect",
                    view.definition().name
                ),
            ));
        };
        let Some(next) = view.part(part_index) else {
            return Err((
                ErrorCode::InvalidPath,
                format!(
                    "part index {part_index} is out of range for `{}` (which has {} part{})",
                    view.definition().name,
                    composite.parts.len(),
                    if composite.parts.len() == 1 { "" } else { "s" }
                ),
            ));
        };
        view = next;
        breadcrumb.push(json!({
            "chip": view.definition().name,
            "path": &instance_path[..=depth],
        }));
    }
    if !view.is_composite() {
        return Err((
            ErrorCode::InvalidPath,
            format!(
                "chip `{}` is built-in and has no internal diagram",
                view.definition().name
            ),
        ));
    }
    Ok((breadcrumb, view))
}

fn values_json(
    circuit: &Circuit,
    view: InstanceView<'_>,
    revision: u64,
    instance_path: &[usize],
) -> ChipReply {
    let def = view.definition();
    let Some(net_bits) = view.net_values() else {
        return Err((
            ErrorCode::InvalidPath,
            format!(
                "chip `{}` is built-in and has no internal diagram",
                def.name
            ),
        ));
    };
    let clocked = circuit.definition().sequential;
    let time = if clocked {
        circuit.time().to_string()
    } else {
        String::new()
    };
    let nets: Vec<Value> = net_bits.into_iter().map(encode_bits).collect();
    let parts: Vec<Value> = view
        .parts()
        .map(|part_view| {
            let inputs: Vec<Value> = part_view
                .input_values()
                .into_iter()
                .map(encode_bits)
                .collect();
            let outputs: Vec<Value> = part_view
                .output_values()
                .into_iter()
                .map(encode_bits)
                .collect();
            json!({
                "inputs": inputs,
                "outputs": outputs,
            })
        })
        .collect();
    Ok(json!({
        "revision": revision,
        "path": instance_path,
        "time": time,
        "clocked": clocked,
        "nets": nets,
        "parts": parts,
    }))
}

struct RecordingSourceProvider {
    inner: HostSourceProvider,
    sources: Arc<Mutex<HashMap<String, Arc<str>>>>,
}

impl SourceProvider for RecordingSourceProvider {
    fn load_chip(&self, chip_name: &str) -> SourceLookup {
        let lookup = self.inner.load_chip(chip_name);
        if let SourceLookup::Found(ref file) = lookup
            && let Ok(mut sources) = self.sources.lock()
        {
            sources.insert(file.id.clone(), file.text.clone());
        }
        lookup
    }
}

fn load(
    host: &SharedHost,
    file: &str,
    name: &str,
) -> Result<(HardwareMachine, HashMap<String, Arc<str>>), Failure> {
    let sources = Arc::new(Mutex::new(HashMap::new()));
    let provider = RecordingSourceProvider {
        inner: HostSourceProvider::new(host.clone(), path::dir(file)),
        sources: sources.clone(),
    };
    let mut machine =
        HardwareMachine::with_probes(Box::new(provider), Arc::new(BuiltinLibrary::standard()));
    machine
        .load_chip(name, path::file_name(file), Span::default())
        .map_err(|e| e.message)?;
    let captured = sources
        .lock()
        .map(|mut map| std::mem::take(&mut *map))
        .unwrap_or_default();
    Ok((machine, captured))
}

/// A value typed into the panel: decimal (`5`, `-1`), binary (`0b101`,
/// `%B101`) or hexadecimal (`0x1F`, `%X1F`), or a JSON number. It must fit
/// the pin; a negative number is taken in two's complement at the pin's width.
pub(crate) fn parse_value(value: &Value, width: u32) -> Result<u64, Failure> {
    let mask = if width >= 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    let parsed: i128 = match value {
        Value::Number(n) => n
            .as_i64()
            .map(i128::from)
            .ok_or("the value must be a whole number")?,
        Value::Bool(b) => i128::from(*b),
        Value::String(s) => {
            let s = s.trim().replace('_', "");
            let lower = s.to_ascii_lowercase();
            let (digits, radix) = if let Some(rest) = lower.strip_prefix("0b") {
                (rest.to_string(), 2)
            } else if let Some(rest) = lower.strip_prefix("%b") {
                (rest.to_string(), 2)
            } else if let Some(rest) = lower.strip_prefix("0x") {
                (rest.to_string(), 16)
            } else if let Some(rest) = lower.strip_prefix("%x") {
                (rest.to_string(), 16)
            } else if let Some(rest) = lower.strip_prefix("%d") {
                (rest.to_string(), 10)
            } else {
                (lower.clone(), 10)
            };
            i128::from_str_radix(&digits, radix).map_err(|_| {
                format!(
                    "`{s}` is not a number; write decimal (5, -1), binary (0b101) or hex (0x1F)"
                )
            })?
        }
        _ => return Err("the value must be a number".to_string()),
    };
    let min = -(i128::from(mask) / 2 + 1);
    if parsed > i128::from(mask) || parsed < min {
        return Err(format!(
            "{parsed} does not fit in {width} bit{}",
            if width == 1 { "" } else { "s" }
        ));
    }
    Ok((parsed as u64) & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_panel_values() {
        assert_eq!(parse_value(&json!(1), 1), Ok(1));
        assert_eq!(parse_value(&json!("-1"), 16), Ok(0xFFFF));
        assert_eq!(parse_value(&json!("0b1010"), 4), Ok(10));
        assert_eq!(parse_value(&json!("%B1010"), 4), Ok(10));
        assert_eq!(parse_value(&json!("0xFF"), 8), Ok(255));
        assert_eq!(parse_value(&json!(" 65535 "), 16), Ok(65535));
        assert_eq!(parse_value(&json!("-32768"), 16), Ok(0x8000));
        assert!(parse_value(&json!("2"), 1).is_err());
        assert!(parse_value(&json!("65536"), 16).is_err());
        assert!(parse_value(&json!("-32769"), 16).is_err());
        assert!(parse_value(&json!("abc"), 16).is_err());
        assert!(parse_value(&json!(1.5), 16).is_err());
        assert_eq!(parse_value(&json!("-1"), 64), Ok(u64::MAX));
    }

    #[test]
    fn encodes_wide_values_at_53_bit_boundary() {
        assert_eq!(encode_bits(0), json!(0));
        assert_eq!(encode_bits(65535), json!(65535));
        assert_eq!(encode_bits(MAX_SAFE_JSON_INT), json!(MAX_SAFE_JSON_INT));
        assert_eq!(
            encode_bits(MAX_SAFE_JSON_INT + 1),
            json!((MAX_SAFE_JSON_INT + 1).to_string())
        );
        assert_eq!(encode_bits(u64::MAX), json!(u64::MAX.to_string()));
    }

    #[test]
    fn parses_instance_paths() {
        assert_eq!(parse_path(&json!({})), Ok(vec![]));
        assert_eq!(parse_path(&json!({"path": null})), Ok(vec![]));
        assert_eq!(parse_path(&json!({"path": []})), Ok(vec![]));
        assert_eq!(parse_path(&json!({"path": [0, 3, 1]})), Ok(vec![0, 3, 1]));
        assert!(parse_path(&json!({"path": [-1]})).is_err());
        assert!(parse_path(&json!({"path": [1.5]})).is_err());
        assert!(parse_path(&json!({"path": ["0"]})).is_err());
        assert!(parse_path(&json!({"path": "0"})).is_err());
    }

    #[test]
    fn hashes_source_text_with_fnv1a_64() {
        assert_eq!(source_hash(""), "cbf29ce484222325");
        assert_eq!(source_hash("a"), "af63dc4c8601ec8c");
        assert_eq!(source_hash("foobar"), "85944171f73967e8");
        // Documented test vector for TypeScript parity.
        assert_eq!(source_hash("CHIP X {}\n"), "f3235dc7995af525");
        assert_ne!(source_hash("CHIP X {}\r\n"), source_hash("CHIP X {}\n"));
        let unicode_hash = source_hash("// λ\n");
        assert_eq!(unicode_hash.len(), 16);
        assert!(
            unicode_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }
}
