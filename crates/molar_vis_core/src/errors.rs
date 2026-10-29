/// Preserve the underlying causes before converting an error to a UI/log string.
/// Molar's file errors display only context; the handler and OS errors are sources.
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        // FileIoError already ends its filename context with a colon.
        if message.trim_end().ends_with(':') {
            message.push(' ');
        } else {
            message.push_str(": ");
        }
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// Keep the panic message when a worker converts an unwind into a reported error.
#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("panic with a non-string payload")
}

/// JavaScript exceptions use `cause` rather than Rust's `Error::source`.
#[cfg(target_arch = "wasm32")]
pub fn js_error_chain(error: &wasm_bindgen::JsValue) -> String {
    use wasm_bindgen::JsCast;
    let mut current = error.clone();
    let mut seen = Vec::new();
    let mut messages = Vec::new();
    while !current.is_null() && !current.is_undefined() && !seen.contains(&current) {
        let message = if let Some(error) = current.dyn_ref::<js_sys::Error>() {
            String::from(error.to_string())
        } else {
            current
                .as_string()
                .unwrap_or_else(|| format!("{current:?}"))
        };
        messages.push(message);
        seen.push(current.clone());
        current = js_sys::Reflect::get(&current, &"cause".into()).unwrap_or_default();
    }
    if messages.is_empty() {
        format!("{error:?}")
    } else {
        messages.join(": ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Context(&'static str, Box<dyn std::error::Error>);
    impl std::fmt::Display for Context {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for Context {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(self.1.as_ref())
        }
    }

    #[test]
    fn reports_every_cause_for_non_io_errors() {
        let root = "bad".parse::<usize>().unwrap_err();
        let detail = root.to_string();
        let error = Context(
            "calculation failed:",
            Box::new(Context("invalid parameter", Box::new(root))),
        );
        assert_eq!(
            error_chain(&error),
            format!("calculation failed: invalid parameter: {detail}")
        );
    }

    #[test]
    fn preserves_owned_and_static_panic_messages() {
        assert_eq!(panic_message(&"worker detail"), "worker detail");
        assert_eq!(panic_message(&String::from("owned detail")), "owned detail");
        assert_eq!(panic_message(&42), "panic with a non-string payload");
    }
}
