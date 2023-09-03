use v8::Context;
use v8::ContextScope;
use v8::Global;
use v8::HandleScope;
use v8::Isolate;
use v8::Local;

use std::fmt::Write;
use std::cell::RefCell;
use std::rc::Rc;

use crate::utils;
use crate::utils::init::initialize_v8;
use crate::utils::inspect::inspect_v8_value;

use super::JsState;
use super::JsStateRef;

#[derive(Debug, PartialEq)]
pub enum EvalError {
    CompileError,
    RuntimeError,
    ConversionError,
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for EvalError {}

pub struct JsRuntime {
    pub(crate) isolate: v8::OwnedIsolate,
    pub(crate) context: Global<Context>,
}

extern "C" fn promise_reject_callback(message: v8::PromiseRejectMessage) {
    let scope = &mut unsafe { v8::CallbackScope::new(&message) };

    print!("Promise rejected {:?}", message.get_event());

    match message.get_value() {
        None => print!(" value=None"),
        Some(value) => print!(" value=Some({})", value.to_rust_string_lossy(scope)),
    }

    println!(" {:?}", message.get_promise());
}

extern "C" fn message_callback(message: v8::Local<v8::Message>, value: v8::Local<v8::Value>) {
    let scope = &mut unsafe { v8::CallbackScope::new(message) };
    let scope = &mut v8::HandleScope::new(scope);
    let message_str = message.get(scope);

    println!(
        "Message callback {:?} {:?}",
        message_str.to_rust_string_lossy(scope),
        inspect_v8_value(value, scope)
    );
}

fn message_from_worker(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _ret: v8::ReturnValue,
) {
    if !args.length() == 1 {
        println!("postMessage expects 1 argument, got {}", args.length());
        utils::throw_type_error(scope, "postMessage expects 1 argument");
        return;
    }

    let message = args.get(0);
    let message = message.to_object(scope).unwrap();

    let kind = utils::get(scope, message, "kind").to_rust_string_lossy(scope);

    match kind.as_str() {
        "console" => {
            let mut output = String::new();

            let level = utils::get(scope, message, "level").to_rust_string_lossy(scope);

            let date = utils::get(scope, message, "date")
                .integer_value(scope)
                .unwrap_or(0);
            let date = chrono::NaiveDateTime::from_timestamp_millis(date).unwrap();

            let args = utils::get(scope, message, "args");

            let args: Local<'_, v8::Array> = args.try_into().unwrap();

            for i in 0..args.length() {
                let arg = args.get_index(scope, i).unwrap();
                write!(output, " {}", arg.to_rust_string_lossy(scope)).unwrap();
            }

            println!("[{:?}] console.{}:{}", date, level, output);
        }
        _ => {
            println!("Unknown message kind: {}", kind);
        }
    }
}

/// Register callback for onMessage
fn register_message_handler(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _ret: v8::ReturnValue,
) {
    println!("onMessage called {}", args.length());

    let callback = args.get(0);

    if !callback.is_function() {
        utils::throw_type_error(scope, "Arg 0 is not a function");
        return;
    }

    let callback: Local<v8::Function> = match callback.try_into() {
        Ok(callback) => callback,
        Err(_) => {
            utils::throw_type_error(scope, "Arg 0 is not a function");
            return;
        }
    };

    let callback = Global::new(scope, callback);

    let state = scope.get_slot::<JsStateRef>().unwrap();
    let mut state = state.borrow_mut();

    match state.handler.as_mut() {
        Some(_) => {
            println!("Handler already registered");

            drop(state);

            utils::throw_error(scope, "Handler already registered");
        }
        None => {
            println!("Registering handler");
            state.handler = Some(callback);
            return;
        }
    };
}

impl JsRuntime {
    /// Create a new context with default extensions
    pub fn create_init() -> Self {
        initialize_v8();

        let mut rt = {
            let mut isolate = Isolate::new(Default::default());

            isolate.set_capture_stack_trace_for_uncaught_exceptions(false, 0);
            isolate.set_promise_reject_callback(promise_reject_callback);
            isolate.add_message_listener(message_callback);

            let context = {
                let scope = &mut HandleScope::new(&mut isolate);

                let context = Context::new(scope);

                let scope = &mut ContextScope::new(scope, context);

                scope.set_slot(Rc::new(RefCell::new(JsState {
                    handler: None,
                    // timers: Timers::new(),
                })));

                let context = Global::new(scope, context);

                context
            };

            JsRuntime { isolate, context }
        };

        rt.eval(include_str!("../runtime/init.js")).unwrap();
        rt.eval(include_str!("../runtime/atob.js")).unwrap();
        rt.eval(include_str!("../runtime/btoa.js")).unwrap();
        rt.eval(include_str!("../runtime/console.js")).unwrap();
        rt.eval(include_str!("../runtime/navigator.js")).unwrap();
        rt.eval(include_str!("../runtime/events.js")).unwrap();
        rt.eval(include_str!("../runtime/fetch/headers.js"))
            .unwrap();
        rt.eval(include_str!("../runtime/fetch/response.js"))
            .unwrap();
        rt.eval(include_str!("../runtime/fetch/request.js"))
            .unwrap();
        rt.eval(include_str!("../runtime/fetch/fetch-event.js"))
            .unwrap();

        // TODO: Snapshot here

        // Set postMessage handler
        {
            let scope = &mut HandleScope::new(&mut rt.isolate);
            let context = Local::new(scope, &rt.context);
            let global = context.global(scope);
            let scope = &mut ContextScope::new(scope, context);

            let post_message = v8::FunctionTemplate::new(scope, message_from_worker);
            let post_message = post_message.get_function(scope).unwrap();

            let name = v8::String::new(scope, "postMessage").unwrap();
            global.set(scope, name.into(), post_message.into());
        }

        // Set onMessage handler
        {
            let scope = &mut HandleScope::new(&mut rt.isolate);
            let context = Local::new(scope, &rt.context);
            let global = context.global(scope);
            let scope = &mut ContextScope::new(scope, context);

            let on_message = v8::FunctionTemplate::new(scope, register_message_handler);
            let on_message = on_message.get_function(scope).unwrap();

            let name = v8::String::new(scope, "onMessage").unwrap();
            global.set(scope, name.into(), on_message.into());
        }

        // Runtime message handler
        rt.eval(include_str!("../runtime/message.js")).unwrap();

        rt
    }

    /// Evaluate a script
    pub fn eval(&mut self, script: &str) -> Result<String, EvalError> {
        let scope = &mut HandleScope::new(&mut self.isolate);

        let context = Local::new(scope, &self.context);
        let scope = &mut ContextScope::new(scope, context);

        let code = v8::String::new(scope, script).ok_or(EvalError::CompileError)?;
        let script = v8::Script::compile(scope, code, None).ok_or(EvalError::CompileError)?;

        // Run script
        let result = script.run(scope).ok_or(EvalError::RuntimeError)?;

        let result = result.to_string(scope).ok_or(EvalError::ConversionError)?;

        Ok(result.to_rust_string_lossy(scope))
    }

    pub fn send_message<E: super::message::RuntimeMessage>(
        &mut self,
        event: &mut E,
    ) -> Option<Local<v8::Value>> {
        let scope = &mut HandleScope::new(&mut self.isolate);
        let context = Local::new(scope, &self.context);

        event.prepare(scope);

        let result = {
            let scope = &mut ContextScope::new(scope, context);

            // Get handler - State must be dropped before the handler is called
            let handler = {
                let state = scope.get_slot::<JsStateRef>().expect("No state found");
                let state = state.borrow_mut();
                match state.handler.clone() {
                    Some(handler) => handler,
                    None => {
                        println!("No handler registered");
                        return None;
                    }
                }
            };

            // Prepare handler call
            let handler = v8::Local::new(scope, handler);
            let undefined = v8::undefined(scope).into();

            let event = event.to_value(scope);

            // Call handler
            let result = handler.call(scope, undefined, &[event]);

            println!("Event result: {:?}", result);

            result
        };

        result
    }

    pub async fn run_event_loop<'a>(&mut self) {
        let scope = &mut HandleScope::new(&mut self.isolate);
        let context = Local::new(scope, &self.context);
        let scope = &mut ContextScope::new(scope, context);

        loop {
            // tokio::macros::support::poll_fn(|cx| Self::poll_timers(cx, scope)).await;

            scope.perform_microtask_checkpoint();

            // let state = scope.get_slot::<super::JsStateRef>().expect("No state found");

            // // Check if we are done
            // if state.borrow().timers.is_empty() {
            //     break;
            // }

            break;
        }
    }
}
