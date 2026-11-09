#![cfg(target_arch = "wasm32")]

//! Runs the real lexer, parser, semantic analyzer and interpreter on
//! `wasm32-unknown-unknown`.
//!
//! ```sh
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!   cargo test -p pine-integration-tests --target wasm32-unknown-unknown \
//!   --test wasm_runtime
//! ```
//!
//! `pine_builtins::per_bar_variables` reads `SystemTime::now()` for `timenow`,
//! which panics in a browser. The tests below build the per-bar values with a
//! host clock instead — the same contract the OpineRust bridge uses — and then
//! run the real interpreter. Passing here is not proof that the browser UI
//! works; it only proves the runtime path links and runs under wasm.

use pine_core::{DefaultPineOutput, PineVersion, Plot, PlotOutput};
use pine_interpreter::{Interpreter, Series, Value};
use wasm_bindgen_test::wasm_bindgen_test;

fn closes() -> Vec<f64> {
    (0..30)
        .map(|i| 100.0 + (i as f64).sin() * 5.0 + i as f64 * 0.1)
        .collect()
}

fn series(id: &str, value: f64) -> Value<DefaultPineOutput> {
    Value::Series(Series {
        id: id.to_string(),
        current: Box::new(Value::Number(value)),
        history: None,
    })
}

/// The per-bar builtins a browser-safe run needs, with `timenow` from the host.
///
/// Mirrors `pine_builtins::per_bar_variables` but never touches the wall clock.
fn per_bar(close: f64, index: usize, now_ms: f64) -> Vec<(String, Value<DefaultPineOutput>)> {
    let mut barstate = std::collections::HashMap::new();
    for (name, flag) in [
        ("isfirst", index == 0),
        ("islast", index == closes().len() - 1),
        ("isnew", true),
        ("isconfirmed", true),
        ("ishistory", true),
        ("isrealtime", false),
        ("islastconfirmedhistory", index == closes().len() - 1),
    ] {
        barstate.insert(name.to_string(), Value::Bool(flag));
    }
    vec![
        ("open".into(), series("open", close - 1.0)),
        ("high".into(), series("high", close + 1.0)),
        ("low".into(), series("low", close - 2.0)),
        ("close".into(), series("close", close)),
        ("volume".into(), series("volume", 10.0)),
        ("hl2".into(), series("hl2", close)),
        ("hlc3".into(), series("hlc3", close)),
        ("hlcc4".into(), series("hlcc4", close)),
        ("ohlc4".into(), series("ohlc4", close)),
        ("bar_index".into(), Value::Number(index as f64)),
        (
            "barstate".into(),
            Value::Object {
                type_name: "barstate".to_string(),
                fields: std::rc::Rc::new(std::cell::RefCell::new(barstate)),
                call: None,
                value: None,
            },
        ),
        ("timenow".into(), Value::Number(now_ms)),
    ]
}

fn last_plot(source: &str) -> Result<Plot, String> {
    let version = PineVersion::detect(source)
        .map_err(|error| error.to_string())?
        .unwrap_or(PineVersion::V5);
    let tokens = pine_lexer::Lexer::with_version(source, version)
        .tokenize()
        .map_err(|error| error.to_string())?;
    let program = pine_parser::Parser::new(tokens)
        .parse_program()
        .map_err(|error| error.to_string())?;

    let (consts, advances) =
        pine_builtins::register_namespace_objects::<DefaultPineOutput>(version, None, None);
    let advances = advances.pre;
    let mut builtins = consts.clone();
    for (name, value) in per_bar(0.0, 0, 0.0) {
        builtins.insert(name, value);
    }
    let diagnostics = pine_sema::analyze(&program, &builtins, None);
    if let Some(first) = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.severity == pine_diagnostics::Severity::Error)
    {
        return Err(first.message.clone());
    }

    let mut interpreter: Interpreter<DefaultPineOutput> = Interpreter::new();
    interpreter.set_const_variables(consts);
    interpreter.per_bar_advances = advances;
    let mut plot = Plot {
        series: f64::NAN,
        ..Default::default()
    };
    for (index, close) in closes().iter().copied().enumerate() {
        interpreter.current_time = Some(1_700_000_000_000 + index as i64 * 60_000);
        for (name, value) in per_bar(close, index, 1_700_000_000_000.0) {
            if matches!(value, Value::Series(_)) {
                interpreter.advance_series(&name, value);
            } else {
                interpreter.set_variable(&name, value);
            }
        }
        let output = interpreter
            .execute(&program)
            .map_err(|error| error.to_string())?;
        if let Some(first) = output.plots().first() {
            plot = first.clone();
        }
    }
    Ok(plot)
}

#[wasm_bindgen_test]
fn v5_and_v6_run_the_interpreter_on_wasm() {
    for version in [5, 6] {
        let source = format!("//@version={version}\nindicator(\"Wasm\")\nplot(ta.sma(close, 3))\n");
        let plot = last_plot(&source).expect("runs on wasm");
        assert!(plot.series.is_finite(), "last SMA is a number");
    }
}

#[wasm_bindgen_test]
fn semantic_errors_report_on_wasm() {
    let source = "//@version=5\nindicator(\"x\")\nplot(ta.no_such_builtin(close))\n";
    let error = last_plot(source).expect_err("unknown builtin must fail");
    assert!(error.contains("no_such_builtin"), "{error}");
}

#[wasm_bindgen_test]
fn na_propagates_instead_of_panicking() {
    // Bar one has no lookback yet: Pine yields `na` and the run must finish.
    last_plot("//@version=5\nindicator(\"x\")\nplot(close[1])\n")
        .expect("na must not abort the run on wasm");
}

#[wasm_bindgen_test]
fn host_clock_and_strings_work_without_an_os_rng() {
    let source =
        "//@version=6\nindicator(\"x\")\nplot(str.length(\"abcd\") + math.round(2.6) + timenow)\n";
    let plot = last_plot(source).expect("runs on wasm");
    assert_eq!(
        plot.series, 1_700_000_000_007.0,
        "host clock reached the script"
    );
}
