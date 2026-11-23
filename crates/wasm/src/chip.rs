//! An interactive chip: set the inputs, evaluate or clock it, read every pin.

use std::sync::Arc;

use n2t_core::builtins::BuiltinLibrary;
use n2t_core::hdl::resolve::HostSourceProvider;
use n2t_core::host::{SharedHost, path};
use n2t_core::source::Span;
use n2t_simulator::{HardwareMachine, Machine};
use serde_json::{Value, json};

use crate::{Failure, Reply, observed, text};

pub struct ChipSession {
    host: SharedHost,
    file: String,
    name: String,
    machine: HardwareMachine,
}

impl ChipSession {
    /// `{"op":"chip.open","path":…}`.
    pub fn open(host: &SharedHost, request: &Value) -> Result<Self, Failure> {
        let file = path::normalize(text(request, "path")?);
        let name = path::stem(&file).to_string();
        let machine = load(host, &file, &name)?;
        Ok(Self {
            host: host.clone(),
            file,
            name,
            machine,
        })
    }

    pub fn call(&mut self, op: &str, request: &Value) -> Reply {
        match op {
            "chip.state" => {}
            "chip.set" => {
                let pin = text(request, "pin")?;
                let value = &request["value"];
                self.set(pin, value)?;
                if request["eval"].as_bool().unwrap_or(false) {
                    self.circuit()?.eval();
                }
            }
            "chip.eval" => self.circuit()?.eval(),
            "chip.tick" => self.circuit()?.tick().map_err(|e| e.to_string())?,
            "chip.tock" => self.circuit()?.tock().map_err(|e| e.to_string())?,
            "chip.step" => {
                // A whole clock cycle, from whichever phase the clock is in.
                let circuit = self.circuit()?;
                if circuit.time().after_tick {
                    circuit.tock().map_err(|e| e.to_string())?;
                } else {
                    circuit.tick().map_err(|e| e.to_string())?;
                    circuit.tock().map_err(|e| e.to_string())?;
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
                self.machine = load(&self.host, &self.file, &self.name)?;
                if request["keepInputs"].as_bool().unwrap_or(true) {
                    let circuit = self.circuit()?;
                    for (name, bits) in inputs {
                        circuit.set(&name, bits);
                    }
                    circuit.eval();
                }
            }
            "key" => {
                let code = request["code"].as_u64().unwrap_or(0) as u16;
                return Ok(json!({ "accepted": self.machine.set_key(code) }));
            }
            "chip.table" => return self.table(),
            other => return Err(format!("`{other}` is not an operation of a chip")),
        }
        Ok(self.state())
    }

    fn circuit(&mut self) -> Result<&mut n2t_simulator::Circuit, Failure> {
        self.machine
            .circuit_mut()
            .ok_or_else(|| "no chip is loaded".to_string())
    }

    fn set(&mut self, pin: &str, value: &Value) -> Result<(), Failure> {
        let name = self.name.clone();
        let circuit = self.circuit()?;
        let width = circuit
            .definition()
            .interface
            .input(pin)
            .map(|(_, info)| info.width)
            .ok_or_else(|| format!("`{name}` has no input pin named `{pin}`"))?;
        let bits = parse_value(value, width)?;
        circuit.set(pin, bits);
        Ok(())
    }

    /// The truth table of a small combinational chip: every combination of
    /// its inputs with the outputs it produces. The chip's current inputs
    /// are put back afterwards.
    fn table(&mut self) -> Reply {
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
            return Err("a clocked chip has no truth table: its outputs depend on time".into());
        }
        if bits > MAX_INPUT_BITS {
            return Err(format!(
                "the chip has {bits} input bits; a truth table is shown for up to {MAX_INPUT_BITS}"
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
            "clocked": clocked,
            "time": time,
            "pins": observed(&self.machine.observe()),
        })
    }
}

fn load(host: &SharedHost, file: &str, name: &str) -> Result<HardwareMachine, Failure> {
    let mut machine = HardwareMachine::new(
        Box::new(HostSourceProvider::new(host.clone(), path::dir(file))),
        Arc::new(BuiltinLibrary::standard()),
    );
    machine
        .load_chip(name, path::file_name(file), Span::default())
        .map_err(|e| e.message)?;
    Ok(machine)
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
}
