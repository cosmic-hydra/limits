use limits::error::Kind;
use limits::interp::{Limits, Value};
use limits::{run_with_engine, Engine};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

const RENDER_LIMIT: usize = 16 * 1024 * 1024;
const RENDER_ERROR: &str = "<value rendering limit exceeded>";

// Observe allocation requests only on the current test thread and only after
// input construction. The regression is allocation before rejection, so checking
// the returned error alone would also pass with the original implementation.
thread_local! {
    static LARGEST_REQUEST: Cell<Option<usize>> = const { Cell::new(None) };
}

struct ObservedAllocator;

fn record_request(size: usize) {
    let _ = LARGEST_REQUEST.try_with(|largest| {
        if let Some(previous) = largest.get() {
            largest.set(Some(previous.max(size)));
        }
    });
}

unsafe impl GlobalAlloc for ObservedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_request(layout.size());
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_request(layout.size());
        System.alloc_zeroed(layout)
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_request(new_size);
        System.realloc(pointer, layout, new_size)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout)
    }
}

#[global_allocator]
static ALLOCATOR: ObservedAllocator = ObservedAllocator;

fn observe_allocations<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            LARGEST_REQUEST.with(|largest| largest.set(None));
        }
    }
    LARGEST_REQUEST.with(|largest| largest.set(Some(0)));
    let reset = Reset;
    let result = operation();
    let largest = LARGEST_REQUEST.with(|largest| largest.get().unwrap());
    drop(reset);
    (result, largest)
}

#[test]
fn oversized_scalar_is_rejected_before_copying_its_bytes() {
    let value = Value::Str("x".repeat(RENDER_LIMIT + 1).into());
    let (rendered, largest) = observe_allocations(|| value.to_string());
    assert_eq!(rendered, RENDER_ERROR);
    assert!(largest < 1024, "oversized scalar allocated {largest} bytes");
}

#[test]
fn escaped_map_key_never_reserves_beyond_the_render_limit() {
    let map = BTreeMap::from([("\"".repeat(RENDER_LIMIT / 2), Value::Nil)]);
    let value = Value::Map(Rc::new(RefCell::new(map)));
    let (rendered, largest) = observe_allocations(|| value.to_string());
    assert_eq!(rendered, RENDER_ERROR);
    assert!(
        largest <= RENDER_LIMIT,
        "escaped key allocated {largest} bytes before rejection"
    );
}

#[test]
fn exact_byte_limit_remains_valid_for_raw_and_escaped_strings() {
    let text: Rc<str> = "é".repeat(RENDER_LIMIT / 2).into();
    let raw = Value::Str(text.clone());
    let rendered = raw.to_string();
    assert_eq!(rendered.len(), RENDER_LIMIT);
    assert_eq!(rendered, text.as_ref());

    let quoted = Value::List(Rc::new(RefCell::new(vec![Value::Str(
        "\"".repeat((RENDER_LIMIT - 4) / 2).into(),
    )])));
    let (rendered, largest) = observe_allocations(|| quoted.to_string());
    assert_eq!(rendered.len(), RENDER_LIMIT);
    assert!(rendered.starts_with("[\"\\\""));
    assert!(rendered.ends_with("\\\"\"]"));
    assert!(largest <= RENDER_LIMIT);
}

#[test]
fn map_keys_and_nested_strings_keep_rust_debug_escaping() {
    let key = "é🌍\n\"\\\u{85}\u{200d}";
    let text = "\0\t\r\u{7f}\u{202e}";
    let value = Value::Map(Rc::new(RefCell::new(BTreeMap::from([(
        key.to_owned(),
        Value::List(Rc::new(RefCell::new(vec![Value::Str(text.into())]))),
    )]))));
    assert_eq!(value.to_string(), format!("{{{key:?}: [{text:?}]}}"));
}

#[test]
fn escaped_key_limit_is_a_located_runtime_error_in_both_engines() {
    let source = "let key=\"\\\"\";for i in 0..23 {key=key+key;}let m={};m[key]=0;\nprint(m);";
    let mut results = Vec::new();
    for engine in [Engine::Tree, Engine::Vm] {
        let mut output = Vec::new();
        let error = run_with_engine(source, &mut output, Limits::default(), engine).unwrap_err();
        assert!(output.is_empty());
        assert_eq!(error.kind, Kind::Runtime);
        assert_eq!(error.msg, "value rendering limit exceeded");
        assert_eq!((error.span.line, error.span.col), (2, 6));
        results.push(error);
    }
    assert_eq!(results[0], results[1]);
}
