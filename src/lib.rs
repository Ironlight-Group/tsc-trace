#![doc = include_str!("../README.md")]

use std::cell::{Cell, RefCell};
use std::io::{Result, Write};
use std::arch::asm;

/// capacity in number of traces per thread
#[cfg(all(not(feature = "off"), feature = "capacity_1_million"))]
pub const TSC_TRACE_CAPACITY: usize = 1_000_000;

/// capacity in number of traces per thread
#[cfg(all(not(feature = "off"), feature = "capacity_8_million"))]
pub const TSC_TRACE_CAPACITY: usize = 8_000_000;

/// capacity in number of traces per thread
#[cfg(all(not(feature = "off"), feature = "capacity_16_million"))]
pub const TSC_TRACE_CAPACITY: usize = 16_000_000;

/// capacity in number of traces per thread
#[cfg(all(not(feature = "off"), feature = "capacity_32_million"))]
pub const TSC_TRACE_CAPACITY: usize = 32_000_000;

/// capacity in number of traces per thread
#[cfg(all(not(feature = "off"), feature = "capacity_64_million"))]
pub const TSC_TRACE_CAPACITY: usize = 64_000_000;

/// capacity in number of traces per thread
#[cfg(feature = "off")]
pub const TSC_TRACE_CAPACITY: usize = 0;

/// capacity in number of traces per thread
#[cfg(all(
    not(feature = "off"),
    not(feature = "capacity_1_million"),
    not(feature = "capacity_8_million"),
    not(feature = "capacity_16_million"),
    not(feature = "capacity_32_million"),
    not(feature = "capacity_64_million")
))]
pub const TSC_TRACE_CAPACITY: usize = 1_000_000;

const CAPACITY: usize = TSC_TRACE_CAPACITY * 3;

#[cfg(feature = "const_array")]
thread_local! {
    static TSC_TRACE_SPANS: RefCell<[u64; CAPACITY]> = const { RefCell::new([0; CAPACITY]) };
    static TSC_TRACE_INDEX: Cell<usize> = const { Cell::new(0) };
}

#[cfg(not(feature = "const_array"))]
thread_local! {
    static TSC_TRACE_SPANS: RefCell<Vec<u64>> = RefCell::new(Vec::with_capacity(CAPACITY));
    static TSC_TRACE_INDEX: Cell<usize> = const { Cell::new(0) };
}

/// Writes the current thread's array of traces in the format:
///
/// tag,start_rdtsc,stop_rdtsc,stop_minus_start\n
///
/// Stops writing once it encounters a stop_rdtsc of zero,
/// assuming that's an unused portion of the array
pub fn write_traces_csv(writer: &mut impl Write) -> Result<()> {
    let mut res = Ok(());
    TSC_TRACE_SPANS.with(|spans| {
        let spans = spans.borrow();
        for chunk in spans.chunks_exact(3) {
            if let &[tag, start, stop] = chunk {
                if stop == 0 {
                    break;
                }
                if let e @ Err(_) = writeln!(writer, "{tag},{start},{stop},{}", stop - start) {
                    res = e;
                    break;
                }
            }
        }
    });
    res
}

/// Writes the current thread's array of traces in a binary format.
/// This is, in order:
///
/// tag: u64
/// start_rdtsc: u64
/// stop_rdtsc: u64
///
/// There are no delimiters between each field or between traces.
/// Assumes little-endian since this library only works for x86.
/// Unlike print_csv, the difference between stop and start is not calculated.
/// Writes the entire array, even zeroed / unused portions.
///
/// This is suitable for import to Clickhouse via format RowBinary
/// <https://clickhouse.com/docs/en/interfaces/formats#rowbinary>
pub fn write_traces_binary(writer: &mut impl Write) -> Result<()> {
    let mut res = Ok(());
    TSC_TRACE_SPANS.with(|spans| {
        let spans = spans.borrow();
        let bytes: &[u8] = bytemuck::cast_slice(&*spans);
        if let e @ Err(_) = writer.write_all(&bytes) {
            res = e;
        }
    });
    res
}

/// Reads the processor's timestamp counter. If the `"lfence"` feature is enabled, includes lfence instructions before and after.
#[inline(always)]
#[cfg(target_arch = "x86")]
pub fn rdtsc() -> u64 {
    #[cfg(feature = "lfence")]
    use core::arch::x86::_mm_lfence;
    use core::arch::x86::_rdtsc;
    unsafe {
        #[cfg(feature = "lfence")]
        _mm_lfence();
        let r = _rdtsc();
        #[cfg(feature = "lfence")]
        _mm_lfence();
        r
    }
}

/// Reads the processor's timestamp counter. If the `"lfence"` feature is enabled, includes lfence instructions before and after.
#[inline(always)]
#[cfg(target_arch = "x86_64")]
pub fn rdtsc() -> u64 {
    #[cfg(feature = "lfence")]
    use core::arch::x86_64::_mm_lfence;
    use core::arch::x86_64::_rdtsc;
    unsafe {
        #[cfg(feature = "lfence")]
        _mm_lfence();
        let r = _rdtsc();
        #[cfg(feature = "lfence")]
        _mm_lfence();
        r
    }
}

/// Workaround for ARM chips. Does not actually use rdtsc, as it is only supported on x86.
#[inline(always)]
#[cfg(target_arch = "aarch64")]
pub fn rdtsc() -> u64 {
    let r: u64;
    unsafe{
        asm!(
            "mrs x0, cntvct_el0",
            out("x0") r
        );
    }
    r
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
pub fn rdtsc() -> u64 {
    unimplemented!("x86 or x86_64 needed for rdtsc, aarch64 needed for workaround")
}

/// This struct must be public so that the trace_span! macro can make an instance of it in your code.
/// Don't rely on any details of it, use the macro instead.
pub struct TraceSpan {
    tag: u64,
    start: u64,
}

impl TraceSpan {
    /// Do not call this, use the trace_span! macro instead.
    pub fn new(tag: u64) -> Self {
        TraceSpan {
            tag,
            start: rdtsc(),
        }
    }
}

impl Drop for TraceSpan {
    fn drop(&mut self) {
        let stop = rdtsc();
        _insert_trace(self.tag, self.start, stop);
    }
}

/// Must be public for use by the insert_trace! macro.
/// Use that macro instead, don't use this directly.
#[inline(always)]
pub fn _insert_trace(tag: u64, start: u64, stop: u64) {
    TSC_TRACE_INDEX.with(|index| {
        let mut i = index.get();
        if i >= CAPACITY {
            i = 0;
        }

        #[cfg(feature = "const_array")]
        TSC_TRACE_SPANS.with(|spans| {
            let mut spans = spans.borrow_mut();
            spans[i] = tag;
            spans[i + 1] = start;
            spans[i + 2] = stop;
            i += 3;
        });

        #[cfg(not(feature = "const_array"))]
        TSC_TRACE_SPANS.with(|spans| {
            let mut spans = spans.borrow_mut();
            if spans.len() >= CAPACITY {
                spans[i] = tag;
                spans[i + 1] = start;
                spans[i + 2] = stop;
                i += 3;
            } else {
                spans.push(tag);
                spans.push(start);
                spans.push(stop);
                i += 3;
            }
        });

        index.set(i);
    })
}

#[macro_export]
#[cfg(not(feature = "off"))]
/// `trace_span!(tag)` Starts a trace span with the given u64 tag that ends at the end of this scope.
/// Creates a local variable named _tsc_trace_span, so don't use that name yourself.
macro_rules! trace_span {
    ($e:expr) => {
        let _tsc_trace_span = TraceSpan::new(($e) as u64);
    };
}

#[macro_export]
#[cfg(feature = "off")]
macro_rules! trace_span {
    ($e:expr) => {};
}

#[macro_export]
#[cfg(not(feature = "off"))]
/// `insert_trace!(tag, start, stop)`
/// Takes any 3 arbitrary expressions that `as u64` works on,
/// immediately inserts them into the thread local array as if they were a single trace.
macro_rules! insert_trace {
    ($a:expr, $b:expr, $c:expr) => {
        _insert_trace(($a) as u64, ($b) as u64, ($c) as u64);
    };
}

#[macro_export]
#[cfg(feature = "off")]
macro_rules! insert_trace {
    ($a:expr, $b:expr, $c:expr) => {};
}
