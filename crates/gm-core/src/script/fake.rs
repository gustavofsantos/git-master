//! Test doubles for the script engine: a fake `ctx` host that answers from a script of replies and
//! records every call, so the adapter, the runtime and the CLI `test` command run without a window,
//! a repository or a process.

use std::collections::{BTreeMap, VecDeque};

use serde_json::{Value, json};

use super::port::{CtxHost, HostReply};

/// A `CtxHost` that records the calls it gets and answers each one from a queue per function name,
/// then from a default. `now` answers a counter and `random` a fixed value, so a run is repeatable.
#[derive(Default)]
pub struct ScriptedHost {
    /// Every call, in order: the function and its arguments.
    pub calls: Vec<(String, Value)>,
    replies: BTreeMap<String, VecDeque<HostReply>>,
    fallback: Option<HostReply>,
    clock: i64,
}

impl ScriptedHost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The next call of `func` gets `reply`. Replies for one function are used in the order added.
    #[must_use]
    pub fn reply(mut self, func: &str, reply: HostReply) -> Self {
        self.replies
            .entry(func.to_owned())
            .or_default()
            .push_back(reply);
        self
    }

    /// Answers every call that has no queued reply. Without it such a call gets `Value(null)`.
    #[must_use]
    pub fn otherwise(mut self, reply: HostReply) -> Self {
        self.fallback = Some(reply);
        self
    }

    /// The names of the functions called, in order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.calls.iter().map(|(name, _)| name.as_str()).collect()
    }
}

impl CtxHost for ScriptedHost {
    fn call(&mut self, func: &str, args: Value) -> HostReply {
        self.calls.push((func.to_owned(), args));
        if let Some(reply) = self.replies.get_mut(func).and_then(VecDeque::pop_front) {
            return reply;
        }
        match func {
            "now" => {
                self.clock += 1000;
                return HostReply::Value(json!(self.clock));
            }
            "random" => return HostReply::Value(json!(0.5)),
            "block" | "fail" => {
                let reason = self
                    .calls
                    .last()
                    .and_then(|(_, a)| a.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                return HostReply::End(if func == "block" {
                    super::port::HostEnd::Blocked(reason)
                } else {
                    super::port::HostEnd::Failed(reason)
                });
            }
            _ => {}
        }
        self.fallback
            .clone()
            .unwrap_or(HostReply::Value(Value::Null))
    }
}

/// A `ScriptEngine` that runs a Rust closure in place of a VM, so the runtime is tested without
/// Luau: the closure makes the `ctx` calls a script would make and returns how the run ends.
pub struct FnEngine<F>(pub F);

impl<F> super::port::ScriptEngine for FnEngine<F>
where
    F: Fn(&mut dyn CtxHost) -> super::port::RunEnd + Send + Sync,
{
    fn check(&self, _chunk: &str, _source: &str) -> Result<(), Vec<super::port::ScriptDiagnostic>> {
        Ok(())
    }

    fn read_meta(
        &self,
        _chunk: &str,
        _source: &str,
    ) -> Result<Value, Vec<super::port::ScriptDiagnostic>> {
        Ok(Value::Null)
    }

    fn read_data(
        &self,
        _chunk: &str,
        _source: &str,
    ) -> Result<Value, Vec<super::port::ScriptDiagnostic>> {
        Ok(Value::Null)
    }

    fn run(
        &self,
        _request: &super::port::RunRequest<'_>,
        host: &mut dyn CtxHost,
    ) -> super::port::RunEnd {
        (self.0)(host)
    }

    fn read_extension(
        &self,
        _chunk: &str,
        _source: &str,
        _prelude: &str,
    ) -> Result<super::port::ExtensionShape, Vec<super::port::ScriptDiagnostic>> {
        Ok(super::port::ExtensionShape {
            meta: Value::Null,
            slots: Vec::new(),
        })
    }

    fn run_slot(
        &self,
        _request: &super::port::RunRequest<'_>,
        _slot: &str,
        _prelude: &str,
        host: &mut dyn CtxHost,
    ) -> super::port::RunEnd {
        (self.0)(host)
    }
}
