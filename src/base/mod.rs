use v8::Context;
use v8::ContextScope;
use v8::HandleScope;
use v8::Isolate;
use v8::OwnedIsolate;
use v8::{Global, Local};

use std::error::Error;

use crate::inspect::inspect_v8_value;
use crate::utils::initialize_v8;

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

impl Error for EvalError {}

pub trait JsExt {
    //  fn bind(scope: &mut HandleScope, context: Local<Context>) -> ();
    fn bind<'s>(&self, scope: &mut v8::HandleScope<'s>);
}

pub struct JsContext {
    isolate: OwnedIsolate,
    context: Global<Context>,
}

pub struct JsState {
    pub handler: Option<Global<v8::Function>>,
}

impl JsContext {
    /// Create a new context
    pub fn create() -> Self {
        initialize_v8();

        let mut isolate = Isolate::new(Default::default());

        let context = {
            // let mut isolate = &runtime.isolate;
            let scope = &mut HandleScope::new(&mut isolate);

            let context = Context::new(scope);

            let scope = &mut ContextScope::new(scope, context);

            // Remove default console
            {
                let global = context.global(scope);
                let console_key = v8::String::new(scope, "console").unwrap();
                global.delete(scope, console_key.into());
            }

            scope.set_slot(JsState { handler: None });

            let context = Global::new(scope, context);

            context
        };

        JsContext { isolate, context }
    }

    /// Create a new context with default extensions
    pub fn create_init() -> JsContext {
        let mut context = JsContext::create();

        context.register(&crate::console::ConsoleExt);
        context.register(&crate::base64_utils::Base64UtilsExt);
        context.register(&crate::event_listener::EventListerExt);

        context
    }

    /// Register a new extension
    pub fn register<E: JsExt>(&mut self, ext: &E) {
        let scope = &mut HandleScope::new(&mut self.isolate);
        let context = Local::new(scope, &self.context);
        let scope = &mut ContextScope::new(scope, context);

        ext.bind(scope);
    }

    pub fn last_exception(&mut self) -> Option<String> {
        None // TODO
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

    /// Call fetch event handler
    pub fn fetch(&mut self) -> Option<String> {
        let scope = &mut HandleScope::new(&mut self.isolate);

        let context = Local::new(scope, &self.context);
        let scope = &mut ContextScope::new(scope, context);

        // Check if script registered event listeners
        let handler = {
            let state = scope
                .get_slot::<JsState>()
                .expect("Missing runtime data in V8 context");

            match &state.handler {
                Some(handler) => Some(handler.clone()),
                None => {
                    println!("No handler registered");
                    None
                }
            }
        };

        if handler.is_none() {
            return None;
        }

        let handler = Local::new(scope, handler.unwrap());
        let undefined = v8::undefined(scope).into();
        let result = handler.call(scope, undefined, &[undefined]).unwrap();
        println!("event result: {:?}", inspect_v8_value(result, scope));
        Some(result.to_string(scope).unwrap().to_rust_string_lossy(scope))
    }
}

#[cfg(test)]
mod tests {
    use crate::base::EvalError;
    use crate::base::JsContext;

    fn prepare_context() -> JsContext {
        JsContext::create()
    }

    /// The default context should have default console removed
    #[test]
    fn console_should_not_be_defined() {
        let mut ctx = prepare_context();

        assert_eq!(
            ctx.eval("typeof console").unwrap(),
            String::from("undefined")
        );
    }

    /// eval should not panic when js exception is thrown
    #[test]
    fn eval_should_not_panic_on_runtime_error() {
        let mut ctx = prepare_context();

        let result = ctx.eval("throw new Error('test')");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), EvalError::RuntimeError);
    }

    /// eval should not panic when js exception is thrown
    #[test]
    fn eval_should_not_panic_on_compile_error() {
        let mut ctx = prepare_context();

        let result = ctx.eval("}");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), EvalError::CompileError);
    }

    #[test]
    fn eval_should_not_panic_on_dynamic_import() {
        let mut ctx = prepare_context();

        let result = ctx.eval("import('moduleName')");

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), EvalError::CompileError);
    }
}
