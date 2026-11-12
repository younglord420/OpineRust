// Re-export all public types from sub-crates
pub use pine_ast as ast;
pub use pine_broker as broker;
pub use pine_builtins as builtins;
use pine_builtins::DefaultPineOutput;
pub use pine_core as core;
pub use pine_data as data;
pub use pine_diagnostics as diagnostics;
pub use pine_format as format;
pub use pine_interpreter as interpreter;
pub use pine_lexer as lexer;
pub use pine_lint as lint;
pub use pine_parser as parser;
pub use pine_sema as sema;

mod run;

pub use pine_broker::{Backtest, Metrics};
pub use pine_core::{DataProvider, DirLoader, FileResolver, LibraryLoader};
pub use run::{Run, RunResult};

use pine_ast::Program;
use pine_core::{Bar, Data, PineVersion, Timeframe, VersionError};
use pine_core::{FullPineOutput, PineOutput};
use pine_diagnostics::Diagnostic;
use pine_interpreter::{Interpreter, RuntimeError, Value};
use pine_lexer::{Lexer, LexerError};
use pine_parser::{Parser, ParserError};
use std::collections::HashMap;
use std::rc::Rc;

/// Error type for Pine operations
#[derive(Debug)]
pub enum Error {
    Lexer(LexerError),
    Parser(ParserError),
    Runtime(RuntimeError),
    /// Semantic analysis failed; the program is invalid. Carries every
    /// diagnostic found.
    Sema(Vec<Diagnostic>),
    /// The script's `//@version=N` annotation names a version this toolchain
    /// cannot compile.
    Version(VersionError),
    /// No bars to run over: neither data nor a provider was given, or the
    /// provider could not produce the requested feed.
    Data(pine_core::ProviderError),
}

impl Error {
    /// The 1-based `(line, column)` an editor should point at, when the error
    /// carries a position. Version errors have none.
    pub fn location(&self) -> Option<(u32, u32)> {
        match self {
            Error::Lexer(e) => Some(e.location()),
            Error::Parser(e) => Some(e.location()),
            _ => None,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Lexer(e) => write!(f, "Lexer error: {}", e),
            Error::Parser(e) => write!(f, "Parser error: {}", e),
            Error::Runtime(e) => write!(f, "Runtime error: {}", e),
            Error::Version(e) => write!(f, "Version error: {}", e),
            Error::Data(e) => write!(f, "Data error: {}", e),
            // One diagnostic per line, so multiple errors are simply appended.
            Error::Sema(diags) => {
                for (i, d) in diags.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{}", d)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<RuntimeError> for Error {
    fn from(e: RuntimeError) -> Self {
        Error::Runtime(e)
    }
}

impl From<LexerError> for Error {
    fn from(e: LexerError) -> Self {
        Error::Lexer(e)
    }
}

impl From<ParserError> for Error {
    fn from(e: ParserError) -> Self {
        Error::Parser(e)
    }
}

impl From<VersionError> for Error {
    fn from(e: VersionError) -> Self {
        Error::Version(e)
    }
}

pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: sema::SymbolTable,
}

/// Parse, semantically analyze and lint `source`.
pub fn analyze(source: &str, loader: Option<&dyn LibraryLoader>) -> Result<Analysis, Error> {
    let version = PineVersion::detect(source)?.unwrap_or(PineVersion::LATEST);
    let tokens = Lexer::with_version(source, version).tokenize()?;
    let program = Parser::new(tokens).parse_program()?;

    let (mut env, _): (HashMap<String, Value<DefaultPineOutput>>, _) =
        pine_builtins::register_namespace_objects(version, None, None);
    for (name, value) in pine_builtins::per_bar_variables(&Bar::default(), None) {
        env.insert(name, value);
    }

    let (mut diagnostics, symbols) = pine_sema::analyze_with_symbols(&program, &env, loader);
    diagnostics.extend(pine_lint::lint(&program));
    diagnostics.sort_by_key(|d| d.pos.unwrap_or((u32::MAX, u32::MAX)));
    Ok(Analysis {
        diagnostics,
        symbols,
    })
}

/// The diagnostics from [`analyze`].
pub fn check(source: &str, loader: Option<&dyn LibraryLoader>) -> Result<Vec<Diagnostic>, Error> {
    Ok(analyze(source, loader)?.diagnostics)
}

/// Collect a program's declared metadata — `indicator`/`library`, `input.*`, and
/// any custom records the output type keeps — without market data.
pub fn decode_metadata<O>(
    source: &str,
    custom_variables: HashMap<String, Value<O>>,
) -> Result<O, Error>
where
    O: FullPineOutput,
{
    let (program, version) = parse_program(source)?;
    let (mut consts, _) = pine_builtins::register_namespace_objects(version, None, None);
    for (name, value) in custom_variables {
        consts.insert(name, value);
    }
    let mut interpreter = Interpreter::<O>::new();
    interpreter.set_const_variables(consts);

    let _ = interpreter.execute(&program);
    Ok(interpreter.output)
}

/// Parse and lint `source`, returning only the lint findings (no semantic
/// analysis). `// @skip(...)` directives are honored.
pub fn lint_source(source: &str) -> Result<Vec<Diagnostic>, Error> {
    let version = PineVersion::detect(source)?.unwrap_or(PineVersion::LATEST);
    let tokens = Lexer::with_version(source, version).tokenize()?;
    let program = Parser::new(tokens).parse_program()?;
    Ok(pine_lint::lint(&program))
}

/// Decode `input.*` overrides from a JSON object — `{"Length": 20, "Smooth":
/// true, "Source": "close"}` — keyed by input title, for
/// [`ScriptBuilder::with_inputs`].
pub fn inputs_from_json(
    json: &str,
) -> Result<HashMap<String, pine_core::InputValue>, serde_json::Error> {
    serde_json::from_str(json)
}

pub struct ScriptBuilder<O: PineOutput> {
    source: String,
    custom_variables: HashMap<String, Value<O>>,
    inputs: HashMap<String, pine_core::InputValue>,
    library_loader: Option<Box<dyn LibraryLoader>>,
    request_provider: Option<Box<dyn DataProvider>>,
    ticker: Option<String>,
    timeframe: Timeframe,
    data: Option<Data>,
    bar_count: Option<usize>,
    broker_factory: Option<Box<dyn pine_broker::BrokerFactory>>,
}

impl<O: PineOutput> ScriptBuilder<O> {
    pub fn with_code(source: &str) -> ScriptBuilder<O> {
        Self {
            source: source.to_string(),
            custom_variables: HashMap::new(),
            inputs: HashMap::new(),
            library_loader: None,
            request_provider: None,
            ticker: None,
            timeframe: Timeframe::default(),
            data: None,
            bar_count: None,
            broker_factory: None,
        }
    }

    /// Host overrides for the script's `input.*` calls, keyed by input title.
    /// Each `input.*` returns (and validates) the override for its title if one
    /// is present, else its declared default. See [`inputs_from_json`].
    pub fn with_inputs(mut self, inputs: HashMap<String, pine_core::InputValue>) -> Self {
        self.inputs = inputs;
        self
    }

    /// Host-supplied variables the script can reference, registered as consts
    /// alongside the builtin namespaces.
    pub fn with_custom_variables(mut self, variables: HashMap<String, Value<O>>) -> Self {
        self.custom_variables = variables;
        self
    }

    /// Resolves `import` statements. Without one, importing a library fails.
    pub fn with_library_loader(mut self, loader: Box<dyn LibraryLoader>) -> Self {
        self.library_loader = Some(loader);
        self
    }

    /// Supplies bars for `request.security`. Without one, `request.security`
    /// returns na.
    pub fn with_request_provider(mut self, provider: Box<dyn DataProvider>) -> Self {
        self.request_provider = Some(provider);
        self
    }

    /// Swaps the broker a `strategy` trades against. Without one, the built-in
    /// [`DefaultBrokerFactory`](pine_broker::DefaultBrokerFactory) is used.
    pub fn with_broker(mut self, factory: Box<dyn pine_broker::BrokerFactory>) -> Self {
        self.broker_factory = Some(factory);
        self
    }

    pub fn with_ticker(mut self, ticker: String) -> Self {
        self.ticker = Some(ticker);
        self
    }

    /// The chart timeframe exposed to the script as `timeframe.*`. Without one,
    /// the namespace is populated with defaults.
    pub fn with_timeframe(mut self, timeframe: Timeframe) -> Self {
        self.timeframe = timeframe;
        self
    }

    /// Run over only the last `bar_count` bars of the feed. Without one, the
    /// whole feed is used.
    pub fn with_bar_count(mut self, bar_count: usize) -> Self {
        self.bar_count = Some(bar_count);
        self
    }

    /// The market to run over: the bars, and the symbol and timeframe they
    /// belong to.
    ///
    /// The data describes itself, so it fills in `syminfo.*` and `timeframe.*`
    /// too. An explicit [`ScriptBuilder::with_syminfo`] or
    /// [`ScriptBuilder::with_timeframe`] still wins, whichever order they are
    /// called in.
    pub fn with_data(mut self, data: Data) -> Self {
        self.data = Some(data);
        self
    }

    /// Compile PineScript source code into a Script with default output
    pub fn compile(self) -> Result<Script<O>, Error>
    where
        O: FullPineOutput,
    {
        let data = match self.data {
            Some(data) => data,
            None => {
                let provider = self
                    .request_provider
                    .as_ref()
                    .ok_or_else(|| Error::Data("no data or request provider set".into()))?;

                let ticker = self.ticker.clone().unwrap_or_default();
                provider
                    .request(&ticker, self.timeframe.clone())
                    .map_err(Error::Data)?
            }
        };

        let syminfo = data.syminfo;
        let timeframe = self.timeframe;

        // Keep only the last `bar_count` bars when the caller limited the run.
        let mut bars = data.bars;
        if let Some(n) = self.bar_count {
            let len = bars.len();
            bars = bars.split_off(len.saturating_sub(n.max(1)));
        }

        // The chart's bar spacing, so `request.security_lower_tf` can reject a
        // request that is not actually lower than the chart timeframe.
        let chart_period = bars
            .windows(2)
            .next()
            .map(|pair| pair[1].time - pair[0].time);

        let source = self.source.as_str();
        let (program, version) = parse_program(source)?;

        let (mut consts, advances) = pine_builtins::register_namespace_objects(
            version,
            Some(syminfo),
            Some(timeframe.clone()),
        );
        for (name, value) in self.custom_variables {
            consts.insert(name, value);
        }

        let mut builtins = consts.clone();
        for (name, value) in pine_builtins::per_bar_variables(&Bar::default(), None) {
            builtins.insert(name, value);
        }

        let errors: Vec<_> =
            pine_sema::analyze(&program, &builtins, self.library_loader.as_deref())
                .into_iter()
                .filter(|diagnostic| diagnostic.severity == pine_diagnostics::Severity::Error)
                .collect();
        if !errors.is_empty() {
            return Err(Error::Sema(errors));
        }

        // Create interpreter and load builtin namespace objects
        let mut interpreter = Interpreter::new();
        interpreter.library_loader = self.library_loader;
        interpreter.request_provider = self.request_provider.map(Rc::from);
        interpreter.chart_period = chart_period;
        interpreter.timeframe = timeframe.clone();
        if let Some(broker_factory) = self.broker_factory {
            interpreter.broker_factory = Some(broker_factory);
        }
        interpreter.set_const_variables(consts);
        interpreter.per_bar_advances = advances.pre;
        interpreter.per_bar_post_advances = advances.post;
        interpreter.inputs = self.inputs;

        Ok(Script {
            program,
            interpreter,
            bars,
        })
    }
}

/// A compiled PineScript program, and the bars it will run over.
///
/// State accumulates across bars — series history, `var` locals, and every
/// stateful builtin's window — exactly as it does in TradingView. That makes a
/// `Script` single-use: [`Script::run`] takes it by value so a second run
/// cannot inherit the first one's state.
pub struct Script<O: PineOutput> {
    program: Program,
    interpreter: Interpreter<O>,
    /// Bars from the builder's source; empty when none was given.
    bars: Vec<Bar>,
}

impl<O: PineOutput> Script<O> {
    /// Run one bar. Private: bars must be replayed in order from the first, so
    /// [`Script::run`] is the only way in.
    pub fn execute(&mut self, bar: &Bar, last_bar: Option<&Bar>) -> Result<O, Error> {
        use interpreter::Value;

        self.interpreter.current_time = Some(bar.time);
        self.interpreter.current_bar = Some(bar.clone());

        for (name, value) in pine_builtins::per_bar_variables(bar, last_bar) {
            if matches!(value, Value::Series(_)) {
                self.interpreter.advance_series(&name, value);
            } else {
                self.interpreter.set_variable(&name, value);
            }
        }
        // `time` and the calendar names stay callable. Their lookback is the
        // number they resolved to on earlier bars, recorded every bar.
        for name in [
            "time",
            "time_close",
            "time_tradingday",
            "bar_index",
            "year",
            "month",
            "dayofmonth",
            "dayofweek",
            "hour",
            "minute",
            "second",
            "weekofyear",
        ] {
            self.interpreter.record_value_lookback(name);
        }

        self.interpreter.execute(&self.program).map_err(Error::from)
    }

    pub fn run_fn<F>(mut self, mut on_output: F) -> Result<(), Error>
    where
        F: FnMut(O),
    {
        let bars = std::mem::take(&mut self.bars);
        let last_bar = bars.last().cloned();
        for bar in &bars {
            let output = self.execute(bar, last_bar.as_ref())?;
            on_output(output);
        }
        Ok(())
    }

    /// Replay the script over every bar from its source, returning what each
    /// one produced.
    pub fn run(mut self) -> Result<Run<O>, Error> {
        let bars = std::mem::take(&mut self.bars);
        let last_bar = bars.last().cloned();
        let outputs = bars
            .iter()
            .map(|bar| self.execute(bar, last_bar.as_ref()))
            .collect::<Result<Vec<O>, Error>>()?;
        let broker = self.interpreter.broker.take();
        Ok(Run { outputs, broker })
    }
}

pub fn execute(source: &str, data: Data) -> Result<(), Error> {
    ScriptBuilder::<DefaultPineOutput>::with_code(source)
        .with_data(data)
        .compile()?
        .run()
        .map(|_| ())
}

fn parse_program(source: &str) -> Result<(Program, PineVersion), Error> {
    let version = PineVersion::detect(source)?.unwrap_or(PineVersion::LATEST);

    let mut lexer = Lexer::with_version(source, version);
    let tokens = lexer.tokenize()?;

    let mut parser = Parser::new(tokens);
    let statements = parser.parse()?;

    Ok((Program::new(statements), version))
}

#[cfg(test)]
mod tests {
    use super::inputs_from_json;
    use pine_core::InputValue;

    /// `+1` is a value, and `inline` only groups settings on a chart.
    #[test]
    fn a_leading_plus_and_an_inline_input_run() {
        use pine_core::{Data, Ohlcv};

        let source = r#"//@version=5
indicator("x")
bull = +1
mode = input.string("a", "Mode", inline = "row", options = ["a", "b"])
plot(bull + (mode == "a" ? 0 : 1))
"#;
        let data = Data::from_ohlcv([Ohlcv {
            time: 1_700_000_000_000,
            open: 1.0,
            high: 2.0,
            low: 0.5,
            close: 1.5,
            volume: 10.0,
        }]);
        super::execute(source, data).expect("unary plus and inline run");
    }

    #[test]
    fn a_line_created_with_bar_time_runs() {
        use pine_core::{Data, Ohlcv};
        let source = r#"//@version=5
indicator("x", overlay = true)
var l = line.new(na, na, na, na, xloc = xloc.bar_time)
l.set_first_point(chart.point.new(time, na, high))
var p = line.new(chart.point.new(time, na, low), chart.point.new(time, na, high), xloc.bar_time, color = color.red)
var q = line.new(chart.point.new(time, na, low), chart.point.new(time, na, high), xloc = xloc.bar_time)
var b = box.new(chart.point.new(time, na, high), chart.point.new(time, na, low), xloc = xloc.bar_time, bgcolor = color.blue)
var t = label.new(chart.point.new(na, bar_index, high), "T", xloc.bar_index, textcolor = color.white)
plot(close)
"#;
        let data = Data::from_ohlcv([Ohlcv {
            time: 1_700_000_000_000,
            open: 1.0,
            high: 2.0,
            low: 0.5,
            close: 1.5,
            volume: 10.0,
        }]);
        super::execute(source, data).expect("bar_time drawings");
    }

    /// A drawing's `xloc` picks the point field. `bar_time` keeps the time when
    /// `chart.point.now()` also stored an index. `bar_index` keeps `na` when the
    /// point has no index, instead of substituting the time or bar 0. A box
    /// born `na` and later filled through `array.get` is the same object.
    #[test]
    fn a_drawing_point_follows_its_xloc() {
        use pine_core::{BoxOutput, Data, LabelOutput, LabelStyle, LineOutput, Ohlcv};

        let source = r#"//@version=5
indicator("points", overlay = true)
var held = box.new(na, na, na, na, xloc = xloc.bar_time, extend = extend.right)
var by_index = box.new(na, na, na, na, xloc = xloc.bar_index)
var boxes = array.new<box>()
var ln = line.new(na, na, na, na, xloc = xloc.bar_time)
var lb = label.new(na, na, "CHoCH", xloc = xloc.bar_time, style = label.style_label_down)
if barstate.isfirst
    array.push(boxes, held)
if barstate.islast
    box got = array.get(boxes, 0)
    got.set_top_left_point(chart.point.new(1000, na, high))
    got.set_bottom_right_point(chart.point.now(low))
    by_index.set_top_left_point(chart.point.new(1000, na, high))
    by_index.set_bottom_right_point(chart.point.new(na, bar_index, low))
    ln.set_first_point(chart.point.now(high))
    ln.set_second_point(chart.point.new(1000, bar_index, low))
    lb.set_point(chart.point.now(close))
"#;
        let data = Data::from_ohlcv([
            Ohlcv {
                time: 1000,
                open: 1.0,
                high: 2.0,
                low: 1.0,
                close: 2.0,
                volume: 1.0,
            },
            Ohlcv {
                time: 2000,
                open: 2.0,
                high: 4.0,
                low: 2.0,
                close: 3.0,
                volume: 1.0,
            },
            Ohlcv {
                time: 3000,
                open: 3.0,
                high: 9.0,
                low: 3.0,
                close: 8.0,
                volume: 1.0,
            },
        ]);
        let run = super::ScriptBuilder::<pine_builtins::DefaultPineOutput>::with_code(source)
            .with_data(data)
            .compile()
            .expect("compiles")
            .run()
            .expect("runs");
        let out = run.outputs.last().expect("a bar");
        let held = out.get_box(0).expect("the bar_time box");
        assert_eq!(
            held.left, 1000.0,
            "bar_time reads the point time, not an index"
        );
        assert_eq!(held.top, 9.0);
        assert_eq!(
            held.right, 3000.0,
            "chart.point.now() on a bar_time box uses the bar time, not bar_index"
        );
        assert_eq!(held.bottom, 3.0);
        assert_eq!(held.extend, "right");
        assert!(held.xloc.contains("time"));

        let by_index = out.get_box(1).expect("the bar_index box");
        assert!(
            by_index.left.is_nan(),
            "a missing index stays na, not 0 and not the time: {}",
            by_index.left
        );
        assert_eq!(by_index.top, 9.0);
        assert_eq!(by_index.right, 2.0, "bar_index reads the index");
        assert_eq!(by_index.bottom, 3.0);

        let line = out.get_line(0).expect("the line");
        assert_eq!(line.x1, 3000.0);
        assert_eq!(line.y1, 9.0);
        assert_eq!(
            line.x2, 1000.0,
            "bar_time ignores the index that chart.point.new stored beside the time"
        );
        assert_eq!(line.y2, 3.0);

        let label = out.get_label(0).expect("the label");
        assert_eq!(label.x, 3000.0);
        assert_eq!(label.y, 8.0);
        assert_eq!(label.text, "CHoCH");
        assert_eq!(label.style, LabelStyle::StyleLabelDown);
    }

    /// `time[n]` keeps every bar, including a subscript that runs only once.
    /// A user type stores that number and still has it on the last bar.
    #[test]
    fn a_conditional_time_lookback_stays_in_a_user_type() {
        use pine_core::{Data, Ohlcv};

        let source = r#"//@version=5
indicator("udt")
type T
    float t
    float prev
var T obj = T.new(na, na)
var float late = na
if bar_index == 2
    obj.t := time
    obj.prev := time[1]
if barstate.islast
    late := time[3]
plot(obj.t, "stored")
plot(obj.prev, "previous")
plot(late, "late")
"#;
        let data = Data::from_ohlcv([
            Ohlcv {
                time: 1000,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
            },
            Ohlcv {
                time: 2000,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
            },
            Ohlcv {
                time: 3000,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
            },
            Ohlcv {
                time: 4000,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
            },
        ]);
        let run = super::ScriptBuilder::<pine_builtins::DefaultPineOutput>::with_code(source)
            .with_data(data)
            .compile()
            .expect("compiles")
            .run()
            .expect("runs");
        let collected = super::RunResult::collect(&run.outputs);
        let stored = collected.plots.get("stored").expect("stored plot");
        let previous = collected.plots.get("previous").expect("previous plot");
        let late = collected.plots.get("late").expect("late plot");
        assert_eq!(stored.last().copied().flatten(), Some(3000.0));
        assert_eq!(previous.last().copied().flatten(), Some(2000.0));
        assert_eq!(
            late.last().copied().flatten(),
            Some(1000.0),
            "time[3] on the only bar that reads it is the first bar"
        );
    }

    /// The LuxAlgo structure script on this machine. It is not part of the
    /// crate: a checkout without the file skips the test. The script itself
    /// stays untracked.
    #[test]
    fn the_structure_script_runs_when_present() {
        use pine_core::{BoxOutput, Data, Ohlcv};
        let path = "/root/egui-charts/smc.txt";
        let Ok(source) = std::fs::read_to_string(path) else {
            return;
        };
        let bars: Vec<Ohlcv> = if let Ok(csv_text) = std::fs::read_to_string("/tmp/btcusdt_1d.csv")
        {
            csv_text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    let parts: Vec<&str> = line.split(',').collect();
                    let t: i64 = parts[0].parse().unwrap();
                    let o: f64 = parts[1].parse().unwrap();
                    let h: f64 = parts[2].parse().unwrap();
                    let l: f64 = parts[3].parse().unwrap();
                    let c: f64 = parts[4].parse().unwrap();
                    let v: f64 = parts[5].parse().unwrap();
                    Ohlcv {
                        time: t,
                        open: o,
                        high: h,
                        low: l,
                        close: c,
                        volume: v,
                    }
                })
                .collect()
        } else {
            (0..240)
                .map(|i| {
                    let wave = ((i as f64) / 12.0).sin();
                    let close = 100.0 + 8.0 * wave + (i as f64) * 0.05;
                    Ohlcv {
                        time: 1_700_000_000_000 + i * 86_400_000,
                        open: close - 1.0,
                        high: close + 2.0,
                        low: close - 2.0,
                        close,
                        volume: 1.0,
                    }
                })
                .collect()
        };
        let run = super::ScriptBuilder::<pine_builtins::DefaultPineOutput>::with_code(&source)
            .with_data(Data::from_ohlcv(bars))
            .compile()
            .expect("structure script compiles")
            .run()
            .expect("structure script runs");
        let out = run.outputs.last().expect("a bar");
        for id in 0..10 {
            if let Some(b) = out.get_box(id) {
                println!(
                    "BOX #{id}: left={} right={} top={} bottom={} extend={} xloc={}",
                    b.left, b.right, b.top, b.bottom, b.extend, b.xloc
                );
            }
        }
    }

    #[test]
    fn intraday_intensity_matches_the_bar_formula() {
        use pine_core::{Data, Ohlcv};
        let source = r#"//@version=5
indicator("iii")
plot(ta.iii, "iii")
"#;
        let data = Data::from_ohlcv([Ohlcv {
            time: 1000,
            open: 1.0,
            high: 10.0,
            low: 0.0,
            close: 10.0,
            volume: 5.0,
        }]);
        let run = super::ScriptBuilder::<pine_builtins::DefaultPineOutput>::with_code(source)
            .with_data(data)
            .compile()
            .expect("compiles")
            .run()
            .expect("runs");
        let iii = super::RunResult::collect(&run.outputs)
            .plots
            .get("iii")
            .and_then(|v| v.last().copied())
            .flatten()
            .expect("iii");
        assert!((iii - 5.0).abs() < 1e-9, "ta.iii = {iii}");
    }

    #[test]
    fn a_source_override_selects_another_series() {
        use pine_core::{Data, InputValue, Ohlcv};
        let source = r#"//@version=5
indicator("src")
src = input.source(close, "Src")
plot(src, "src")
"#;
        let data = Data::from_ohlcv([Ohlcv {
            time: 1000,
            open: 4.0,
            high: 9.0,
            low: 1.0,
            close: 7.0,
            volume: 1.0,
        }]);
        let mut inputs = std::collections::HashMap::new();
        inputs.insert("Src".to_string(), InputValue::Str("open".to_string()));
        let run = super::ScriptBuilder::<pine_builtins::DefaultPineOutput>::with_code(source)
            .with_inputs(inputs)
            .with_data(data)
            .compile()
            .expect("compiles")
            .run()
            .expect("runs");
        let value = super::RunResult::collect(&run.outputs)
            .plots
            .get("src")
            .and_then(|v| v.last().copied())
            .flatten();
        assert_eq!(value, Some(4.0));
    }

    #[test]
    fn a_table_keeps_its_frame_border_and_merge() {
        use pine_core::{Data, Ohlcv, TableOutput};
        let source = r#"//@version=5
indicator("table")
var t = table.new(position.top_left, 2, 2, frame_color = color.red, frame_width = 2, border_color = color.blue, border_width = 1)
if barstate.islast
    table.merge_cells(t, 0, 0, 1, 0)
    table.cell_set_tooltip(t, 0, 1, "tip")
"#;
        let data = Data::from_ohlcv([Ohlcv {
            time: 1000,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
        }]);
        let run = super::ScriptBuilder::<pine_builtins::DefaultPineOutput>::with_code(source)
            .with_data(data)
            .compile()
            .expect("compiles")
            .run()
            .expect("runs");
        let table = run
            .outputs
            .last()
            .and_then(|out| out.get_table(0))
            .expect("table");
        assert_eq!(table.frame_width, 2.0);
        assert_eq!(table.border_width, 1.0);
        assert!(table.frame_color.is_some());
        assert!(table.border_color.is_some());
        assert_eq!(table.merges, vec![(0, 0, 1, 0)]);
        assert_eq!(
            table.cells.get(&(0, 1)).map(|cell| cell.tooltip.as_str()),
            Some("tip")
        );
    }

    #[test]
    fn a_typed_array_constructor_and_natural_log_run() {
        use pine_core::{Data, Ohlcv};
        let source = r#"//@version=5
indicator("more")
boxes = array.new_box(0)
plot(array.size(boxes), "n")
plot(math.ln(math.e), "ln")
"#;
        let data = Data::from_ohlcv([Ohlcv {
            time: 1000,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 1.0,
        }]);
        let run = super::ScriptBuilder::<pine_builtins::DefaultPineOutput>::with_code(source)
            .with_data(data)
            .compile()
            .expect("compiles")
            .run()
            .expect("runs");
        let collected = super::RunResult::collect(&run.outputs);
        assert_eq!(
            collected
                .plots
                .get("n")
                .and_then(|v| v.last().copied())
                .flatten(),
            Some(0.0)
        );
        let ln = collected
            .plots
            .get("ln")
            .and_then(|v| v.last().copied())
            .flatten()
            .expect("ln");
        assert!((ln - 1.0).abs() < 1e-9, "math.ln(math.e) = {ln}");
    }

    #[test]
    fn test_smc_pivots_and_order_blocks_debug() {
        use pine_core::{Data, Ohlcv};
        let source = r#"//@version=5
indicator("debug")
leg(int size) =>
    var leg = 0
    newLegHigh = high[size] > ta.highest(size)
    newLegLow = low[size] < ta.lowest(size)
    if newLegHigh
        leg := 0
    else if newLegLow
        leg := 1
    leg

startOfNewLeg(int l) => ta.change(l) != 0
startOfBullishLeg(int l) => ta.change(l) == +1
startOfBearishLeg(int l) => ta.change(l) == -1

l5 = leg(5)
new_p = startOfNewLeg(l5)
plot(l5, "l5")
plot(new_p ? 1.0 : 0.0, "new_p")
"#;
        let bars: Vec<Ohlcv> = (0..50)
            .map(|i| {
                let wave = ((i as f64) / 4.0).sin();
                let close = 100.0 + 10.0 * wave;
                Ohlcv {
                    time: 1000 + i * 1000,
                    open: close,
                    high: close + 2.0,
                    low: close - 2.0,
                    close,
                    volume: 1.0,
                }
            })
            .collect();
        let run = super::ScriptBuilder::<pine_builtins::DefaultPineOutput>::with_code(source)
            .with_data(Data::from_ohlcv(bars))
            .compile()
            .expect("compiles")
            .run()
            .expect("runs");
        let coll = super::RunResult::collect(&run.outputs);
        let new_p = coll.plots.get("new_p").unwrap();
        let any_new = new_p.iter().filter_map(|&v| v).any(|v| v > 0.5);
        println!(
            "new_p count: {:?}",
            new_p.iter().filter_map(|&v| v).filter(|&v| v > 0.5).count()
        );
        assert!(any_new, "should have new pivots");
    }

    #[test]
    fn decodes_input_overrides_from_json() {
        let map = inputs_from_json(r#"{"Length": 20, "Ratio": 1.5, "On": true, "Mode": "fast"}"#)
            .unwrap();
        assert_eq!(map["Length"], InputValue::Int(20));
        assert_eq!(map["Ratio"], InputValue::Float(1.5));
        assert_eq!(map["On"], InputValue::Bool(true));
        assert_eq!(map["Mode"], InputValue::Str("fast".to_string()));
    }
}
