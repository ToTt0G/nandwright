//! What a test script drives.
//!
//! The script runner owns control flow, output and comparison; a [`Machine`]
//! owns the simulated thing: a chip (Hardware Simulator dialect), a Hack
//! computer (CPU Emulator dialect) or a VM (VM Emulator dialect).

use std::any::Any;
use std::sync::Arc;

use n2t_core::builtins::{BuiltinChip, BuiltinLibrary, MethodParameter, StateShape, WritePolicy};
use n2t_core::hdl::definition::NetKind;
use n2t_core::hdl::resolve::SourceProvider;
use n2t_core::hdl::{Elaborator, ElaboratorOptions};
use n2t_core::host::{SharedHost, path};
use n2t_core::source::Span;
use n2t_core::tst::ast::*;

use crate::circuit::{Circuit, PinHandle};
use crate::format::Value;
use crate::primitives::BuiltinRegistry;
use crate::runner::{RunError, error, load_failure};

/// Where a machine finds the files a script names.
pub struct Files<'a> {
    pub host: &'a SharedHost,
    /// The script's folder, in engine form.
    pub dir: &'a str,
}

/// What kind of thing an observed value is, for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Input,
    Output,
    Internal,
    Register,
    Memory,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Input => "input",
            Role::Output => "output",
            Role::Internal => "internal",
            Role::Register => "register",
            Role::Memory => "memory",
        }
    }
}

/// One named value of a machine at a moment in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub name: String,
    pub role: Role,
    pub width: u32,
    pub bits: u64,
}

pub trait Machine {
    fn dialect(&self) -> Dialect;

    /// Executes `load`.
    fn load(&mut self, files: &Files<'_>, file: &FileName) -> Result<(), RunError>;

    /// Executes a stepping command (`eval`, `tick`, `tock`, `ticktock`,
    /// `vmstep`) or a built-in chip method.
    fn execute(&mut self, files: &Files<'_>, statement: &SimpleStatement) -> Result<(), RunError>;

    fn read(&self, variable: &Variable) -> Result<Value, RunError>;

    fn write(&mut self, variable: &Variable, value: &IntegerLiteral) -> Result<(), RunError>;

    /// The values worth showing a student when something goes wrong: for a
    /// chip, every pin including the internal ones.
    fn observe(&self) -> Vec<Observed>;

    /// Things worth telling the student that are not errors, collected since
    /// the last call (for example: the `.hack` file a script loaded is not
    /// what the `.asm` next to it assembles to).
    fn take_notes(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// Presses (or with `0` releases) a key on the machine's keyboard.
    /// Returns `false` if the machine has no keyboard.
    fn set_key(&mut self, code: u16) -> bool;

    /// Whether `variable` shows what the keyboard holds: the keyboard's
    /// memory cell, or an output pin of a chip that contains a keyboard. A
    /// loop that waits for such a variable to take a value is waiting for a
    /// key; a loop that waits for anything else is not.
    fn reads_keyboard(&self, _variable: &Variable) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// The Hardware Simulator: a chip under test.
pub struct HardwareMachine {
    elaborator: Elaborator<Box<dyn SourceProvider>>,
    registry: BuiltinRegistry,
    circuit: Option<Circuit>,
    loaded: Option<String>,
    probes: bool,
}

impl HardwareMachine {
    pub fn new(provider: Box<dyn SourceProvider>, builtins: Arc<BuiltinLibrary>) -> Self {
        Self {
            elaborator: Elaborator::with_options(provider, builtins, ElaboratorOptions::runtime()),
            registry: BuiltinRegistry::standard(),
            circuit: None,
            loaded: None,
            probes: false,
        }
    }

    /// Creates a hardware simulator whose loaded circuits retain per-instance
    /// probes (see [`Circuit::with_probes`]), for interactive inspection.
    pub fn with_probes(provider: Box<dyn SourceProvider>, builtins: Arc<BuiltinLibrary>) -> Self {
        Self {
            probes: true,
            ..Self::new(provider, builtins)
        }
    }

    pub fn circuit(&self) -> Option<&Circuit> {
        self.circuit.as_ref()
    }

    pub fn circuit_mut(&mut self) -> Option<&mut Circuit> {
        self.circuit.as_mut()
    }

    pub fn loaded_chip(&self) -> Option<&str> {
        self.loaded.as_deref()
    }

    pub fn elaborator(&mut self) -> &mut Elaborator<Box<dyn SourceProvider>> {
        &mut self.elaborator
    }

    pub fn elaborator_ref(&self) -> &Elaborator<Box<dyn SourceProvider>> {
        &self.elaborator
    }

    /// Loads a chip by name, as `load Name.hdl` does.
    pub fn load_chip(&mut self, name: &str, file_name: &str, span: Span) -> Result<(), RunError> {
        let unit = self.elaborator.compile(name);
        let Some(definition) = unit.definition.clone() else {
            return Err(error(load_failure(file_name, &unit), span));
        };
        let circuit = if self.probes {
            Circuit::with_probes(definition, &self.registry)
        } else {
            Circuit::new(definition, &self.registry)
        }
        .map_err(|e| error(e.to_string(), span))?;
        self.circuit = Some(circuit);
        self.loaded = Some(name.to_string());
        Ok(())
    }

    /// Whether the loaded chip contains a keyboard.
    fn has_keyboard(&self) -> bool {
        self.circuit.as_ref().is_some_and(|circuit| {
            circuit
                .primitives()
                .iter()
                .any(|p| matches!(p.symbol.as_str(), "Keyboard" | "Memory"))
        })
    }

    fn circuit_or_error(&mut self, span: Span) -> Result<&mut Circuit, RunError> {
        self.circuit
            .as_mut()
            .ok_or_else(|| error("no chip is loaded; use `load Xxx.hdl` first", span))
    }

    fn find_builtin(
        &self,
        name: &Ident,
        filter: impl Fn(&BuiltinChip) -> bool,
    ) -> Result<(usize, Arc<BuiltinChip>), RunError> {
        let circuit = self
            .circuit
            .as_ref()
            .ok_or_else(|| error("no chip is loaded", name.span))?;
        let library = self.elaborator.builtins();
        let candidates: Vec<(usize, Arc<BuiltinChip>)> = circuit
            .primitives()
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let chip = library.by_symbol(&p.symbol)?;
                (filter(chip) && chip.name == p.chip_name).then(|| (i, chip.clone()))
            })
            .collect();
        let exact: Vec<_> = candidates
            .iter()
            .filter(|(_, c)| c.name == name.name)
            .collect();
        let matching = if exact.is_empty() {
            candidates
                .iter()
                .filter(|(_, c)| c.name.eq_ignore_ascii_case(&name.name))
                .collect()
        } else {
            exact
        };
        match matching.as_slice() {
            [] => Err(error(
                format!("the loaded chip contains no built-in `{}`", name.name),
                name.span,
            )),
            [one] => Ok((*one).clone()),
            [first, ..] => Err(error(
                format!(
                    "the loaded chip contains {} instances of built-in `{}`; the reference is ambiguous",
                    matching.len(),
                    first.1.name
                ),
                name.span,
            )),
        }
    }

    fn resolve(&self, variable: &Variable) -> Result<Handle, RunError> {
        let span = variable.span;
        if variable.is_time() {
            return Ok(Handle::Time);
        }
        let circuit = self
            .circuit
            .as_ref()
            .ok_or_else(|| error("no chip is loaded; use `load Xxx.hdl` first", span))?;
        if let Some(index) = variable.index {
            let (instance, chip) =
                self.find_builtin(&variable.name, |c| c.script_state.is_some())?;
            let Some(state) = chip.script_state.as_ref() else {
                return Err(error(
                    format!("`{}` has no script-visible state", chip.name),
                    span,
                ));
            };
            let index = match (state.shape, index.value) {
                (StateShape::Scalar, None | Some(0)) => 0,
                (StateShape::Indexed { len }, Some(i)) if i < len as u64 => i as usize,
                _ => {
                    return Err(error(
                        format!("invalid index for `{}`", chip.name),
                        index.span,
                    ));
                }
            };
            return Ok(Handle::State {
                chip: chip.name.clone(),
                instance,
                index,
                width: state.width,
                write: state.write,
            });
        }
        let name = &variable.name.name;
        let pins = circuit.pins();
        if let Some(pin) = pins.iter().find(|p| p.name == *name) {
            return Ok(Handle::Pin(pin.clone()));
        }
        let folded: Vec<&PinHandle> = pins
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .collect();
        match folded.as_slice() {
            [one] => Ok(Handle::Pin((*one).clone())),
            [] => Err(error(
                format!("the loaded chip has no pin named `{name}`"),
                span,
            )),
            _ => Err(error(
                format!("`{name}` is ambiguous; use the exact spelling"),
                span,
            )),
        }
    }

    fn builtin_method(
        &mut self,
        files: &Files<'_>,
        target: &Ident,
        method: &Ident,
        arguments: &[Argument],
        span: Span,
    ) -> Result<(), RunError> {
        let (instance, chip) = self.find_builtin(target, |c| !c.methods.is_empty())?;
        let Some(descriptor) = chip.method(&method.name) else {
            return Err(error(
                format!("built-in `{}` has no method `{}`", chip.name, method.name),
                method.span,
            ));
        };
        if arguments.len() != descriptor.parameters.len() {
            return Err(error(
                format!(
                    "`{} {}` takes {} argument(s)",
                    chip.name,
                    descriptor.name,
                    descriptor.parameters.len()
                ),
                span,
            ));
        }
        // The only standard method is `ROM32K load file`.
        let ([MethodParameter::FileName { .. }], [argument]) =
            (descriptor.parameters.as_slice(), arguments)
        else {
            return Err(error(
                format!("method `{}` is not supported", descriptor.name),
                method.span,
            ));
        };
        let name = match &argument.kind {
            ArgumentKind::Word(w) | ArgumentKind::String(w) => w.clone(),
            ArgumentKind::Percent { .. } => {
                return Err(error("expected a file name", argument.span));
            }
        };
        if let Some(problem) = n2t_core::tst::parser::check_basename(&name) {
            return Err(error(problem, argument.span));
        }
        let file = path::join(files.dir, &name);
        let words = crate::program::load_program(files.host, &file)
            .map_err(|message| error(message, argument.span))?;
        let circuit = self.circuit_or_error(span)?;
        if !circuit.load_program(instance, &words) {
            return Err(error(
                format!("cannot load `{name}` into `{}`", chip.name),
                argument.span,
            ));
        }
        Ok(())
    }
}

/// The address of the keyboard's memory cell.
pub(crate) const KEYBOARD_ADDRESS: u64 = 24576;

/// `RAM[24576]`, as the CPU Emulator and VM Emulator scripts name it.
pub(crate) fn is_keyboard_cell(variable: &Variable) -> bool {
    variable.name.name.eq_ignore_ascii_case("RAM")
        && variable
            .index
            .and_then(|i| i.value)
            .is_some_and(|i| i == KEYBOARD_ADDRESS)
}

#[derive(Debug, Clone)]
enum Handle {
    Time,
    Pin(PinHandle),
    State {
        chip: String,
        instance: usize,
        index: usize,
        width: u32,
        write: WritePolicy,
    },
}

impl Machine for HardwareMachine {
    fn dialect(&self) -> Dialect {
        Dialect::Hardware
    }

    fn load(&mut self, _files: &Files<'_>, file: &FileName) -> Result<(), RunError> {
        if !file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("hdl"))
        {
            return Err(error(
                "the Hardware Simulator loads chips: `load` needs a `.hdl` file",
                file.span,
            ));
        }
        let name = file.stem().to_string();
        self.load_chip(&name, &file.name, file.span)
    }

    fn execute(&mut self, files: &Files<'_>, statement: &SimpleStatement) -> Result<(), RunError> {
        let span = statement.command_span;
        match &statement.command {
            Command::Eval => self.circuit_or_error(span)?.eval(),
            Command::Tick => self
                .circuit_or_error(span)?
                .tick()
                .map_err(|e| error(e.to_string(), span))?,
            Command::Tock => self
                .circuit_or_error(span)?
                .tock()
                .map_err(|e| error(e.to_string(), span))?,
            Command::BuiltinMethod {
                target,
                method,
                arguments,
            } => self.builtin_method(files, target, method, arguments, span)?,
            other => {
                return Err(error(
                    format!(
                        "`{}` is not a Hardware Simulator command; a chip is stepped with `eval`, or `tick` and `tock`",
                        other.name()
                    ),
                    statement.keyword.span,
                ));
            }
        }
        Ok(())
    }

    fn read(&self, variable: &Variable) -> Result<Value, RunError> {
        let handle = self.resolve(variable)?;
        let Some(circuit) = &self.circuit else {
            return Ok(Value::Text(String::new()));
        };
        Ok(match handle {
            Handle::Time => Value::Text(circuit.time().to_string()),
            Handle::Pin(pin) => Value::Int {
                bits: circuit.read(&pin),
                width: pin.width,
            },
            Handle::State {
                instance,
                index,
                width,
                ..
            } => Value::Int {
                bits: circuit.read_state(instance, index).unwrap_or(0),
                width,
            },
        })
    }

    fn write(&mut self, variable: &Variable, value: &IntegerLiteral) -> Result<(), RunError> {
        let handle = self.resolve(variable)?;
        let span = variable.span;
        match handle {
            Handle::Time => Err(error("`time` is read-only", span)),
            Handle::Pin(pin) => {
                if !matches!(pin.kind, NetKind::Input(_)) {
                    return Err(error(
                        format!("`{variable}` is not an input pin and cannot be set"),
                        span,
                    ));
                }
                let bits = value.encode(pin.width).map_err(|m| error(m, value.span))?;
                self.circuit_or_error(span)?.write(&pin, bits);
                Ok(())
            }
            Handle::State {
                chip,
                instance,
                index,
                width,
                write,
            } => {
                if write == WritePolicy::ReadOnly {
                    return Err(error(format!("the state of `{chip}` is read-only"), span));
                }
                let bits = value.encode(width).map_err(|m| error(m, value.span))?;
                let circuit = self.circuit_or_error(span)?;
                if !circuit.write_state(instance, index, bits) {
                    return Err(error(format!("cannot write `{variable}`"), span));
                }
                Ok(())
            }
        }
    }

    fn observe(&self) -> Vec<Observed> {
        let Some(circuit) = &self.circuit else {
            return Vec::new();
        };
        circuit
            .pins()
            .iter()
            .map(|pin| Observed {
                name: pin.name.clone(),
                role: match pin.kind {
                    NetKind::Input(_) => Role::Input,
                    NetKind::Output(_) => Role::Output,
                    NetKind::Internal => Role::Internal,
                },
                width: pin.width,
                bits: circuit.read(pin),
            })
            .collect()
    }

    fn set_key(&mut self, code: u16) -> bool {
        let has_keyboard = self.has_keyboard();
        if has_keyboard && let Some(circuit) = &mut self.circuit {
            circuit.set_key(code);
        }
        has_keyboard
    }

    fn reads_keyboard(&self, variable: &Variable) -> bool {
        if !self.has_keyboard() {
            return false;
        }
        match self.resolve(variable) {
            Ok(Handle::Pin(pin)) => matches!(pin.kind, NetKind::Output(_)),
            Ok(Handle::State { chip, .. }) => chip == "Keyboard",
            _ => false,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
