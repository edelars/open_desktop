//! Host audio output (S11-41): lock-free block ring, real-time consumer, CoreAudio device output and
//! an offline WAV writer ([AUDIO_DEVICE.md](../../../docs/specs/AUDIO_DEVICE.md) sections 5 and 6).
//!
//! The device model (S11-42) is the only producer: it hands over 480-frame blocks of 48 kHz, 16-bit,
//! interleaved stereo audio through [AudioProducer] without blocking and without allocating. The
//! consumer is [AudioConsumer]; the CoreAudio callback ([DeviceOutput]) and the offline renderer
//! ([WavFileOutput]) are both thin drivers of [AudioConsumer::fill]. The trait `AudioSink` of `od-machine` does not
//! exist yet (its signature is S11-42); [AudioProducer] has the semantics section 5 prescribes and is
//! what that trait will be implemented over. No Rust-to-Swift audio ABI exists: ring and consumer
//! live in this crate.
//!
//! The ring sizes (16 blocks, prime at 4) are a hypothesis until measured; see the notes in
//! AUDIO_DEVICE.md section 10.

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Sample rate of the only supported format, in Hz.
pub const SAMPLE_RATE_HZ: u32 = 48_000;
/// Interleaved channels of the only supported format (stereo).
pub const CHANNELS: usize = 2;
/// Frames in one transfer block: 10 ms at 48 kHz.
pub const BLOCK_FRAMES: usize = 480;
/// `i16` samples in one block (960, 1 920 bytes).
pub const BLOCK_SAMPLES: usize = BLOCK_FRAMES * CHANNELS;
/// Ring capacity in blocks (160 ms). Hypothesis until measured.
pub const RING_BLOCKS: usize = 16;
/// Blocks that must be queued before the consumer starts to output (40 ms). Hypothesis until measured.
pub const PRIME_BLOCKS: usize = 4;

/// One transfer block: `BLOCK_FRAMES` frames, interleaved L/R, signed 16-bit.
pub type AudioBlock = [i16; BLOCK_SAMPLES];

/// Outcome of [AudioProducer::write_block].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteResult {
    Accepted,
    /// The ring was full (overrun): the new block was discarded and counted in `dropped_blocks`.
    Dropped,
}

/// Counters of the audio output, readable from any thread ("STATS" of VMHost; the line format is open).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AudioStats {
    /// Blocks accepted by the ring.
    pub blocks_written: u64,
    /// Blocks discarded because the ring was full (overrun).
    pub dropped_blocks: u64,
    /// Blocks taken from the ring by the consumer and played.
    pub blocks_played: u64,
    /// Silent blocks output because the primed ring was empty.
    pub underruns: u64,
    /// Silent blocks output while the ring is not primed yet (start or after a flush); not an underrun.
    pub prefill_silent_blocks: u64,
    /// Calls of [AudioProducer::flush].
    pub flushes: u64,
}

struct Shared {
    slots: Box<[UnsafeCell<AudioBlock>]>,
    /// Blocks consumed so far; written only by the consumer.
    head: AtomicU64,
    /// Blocks produced so far; written only by the producer.
    tail: AtomicU64,
    /// Value of `tail` at the last flush; blocks before it are to be discarded.
    flush_mark: AtomicU64,
    flush_count: AtomicU64,
    blocks_written: AtomicU64,
    dropped_blocks: AtomicU64,
    blocks_played: AtomicU64,
    underruns: AtomicU64,
    prefill_silent_blocks: AtomicU64,
    flushes: AtomicU64,
}

// SAFETY: the slots are the only non-Sync part. Block `n` lives in slot `n % RING_BLOCKS`. The producer writes a
// slot only while `tail - head < RING_BLOCKS` (it loaded `head` with Acquire), so the slot is outside the
// consumer's window `[head, tail)`; it publishes it with a Release store of `tail`. The consumer reads
// only slots of its window (`tail` loaded with Acquire) and copies one out completely before the Release
// store of `head` that hands the slot back. There is exactly one producer and one consumer
// handle (neither is `Clone`, both methods take `&mut self`).
unsafe impl Sync for Shared {}

impl Shared {
    fn stats(&self) -> AudioStats {
        AudioStats {
            blocks_written: self.blocks_written.load(Ordering::Relaxed),
            dropped_blocks: self.dropped_blocks.load(Ordering::Relaxed),
            blocks_played: self.blocks_played.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
            prefill_silent_blocks: self.prefill_silent_blocks.load(Ordering::Relaxed),
            flushes: self.flushes.load(Ordering::Relaxed),
        }
    }
}

/// Read-only handle to the counters; clone freely.
#[derive(Clone)]
pub struct AudioStatsHandle(Arc<Shared>);

impl AudioStatsHandle {
    pub fn snapshot(&self) -> AudioStats {
        self.0.stats()
    }
}

/// The ring split into its three handles.
pub struct AudioRing {
    pub producer: AudioProducer,
    pub consumer: AudioConsumer,
    pub stats: AudioStatsHandle,
}

/// Creates an empty ring (the single allocation of the audio path).
pub fn audio_ring() -> AudioRing {
    let slots: Box<[UnsafeCell<AudioBlock>]> = (0..RING_BLOCKS)
        .map(|_| UnsafeCell::new([0; BLOCK_SAMPLES]))
        .collect();
    let shared = Arc::new(Shared {
        slots,
        head: AtomicU64::new(0),
        tail: AtomicU64::new(0),
        flush_mark: AtomicU64::new(0),
        flush_count: AtomicU64::new(0),
        blocks_written: AtomicU64::new(0),
        dropped_blocks: AtomicU64::new(0),
        blocks_played: AtomicU64::new(0),
        underruns: AtomicU64::new(0),
        prefill_silent_blocks: AtomicU64::new(0),
        flushes: AtomicU64::new(0),
    });
    AudioRing {
        producer: AudioProducer {
            shared: Arc::clone(&shared),
            tail: 0,
        },
        consumer: AudioConsumer {
            shared: Arc::clone(&shared),
            head: 0,
            seen_flush: 0,
            primed: false,
            current: [0; BLOCK_SAMPLES],
            pos: BLOCK_SAMPLES,
        },
        stats: AudioStatsHandle(shared),
    }
}

/// The single producer of the ring (the device model's thread). Neither method blocks, allocates or
/// makes a system call.
pub struct AudioProducer {
    shared: Arc<Shared>,
    tail: u64,
}

impl AudioProducer {
    /// Queues one block. A full ring discards the block ([WriteResult::Dropped]); the guest position
    /// keeps running regardless, so the guest sees no difference.
    pub fn write_block(&mut self, block: &AudioBlock) -> WriteResult {
        let head = self.shared.head.load(Ordering::Acquire);
        if self.tail - head >= RING_BLOCKS as u64 {
            self.shared.dropped_blocks.fetch_add(1, Ordering::Relaxed);
            return WriteResult::Dropped;
        }
        let slot = &self.shared.slots[(self.tail % RING_BLOCKS as u64) as usize];
        // SAFETY: see the `Sync` impl of `Shared`: this slot is outside the consumer's window.
        unsafe { *slot.get() = *block };
        self.tail += 1;
        self.shared.tail.store(self.tail, Ordering::Release);
        self.shared.blocks_written.fetch_add(1, Ordering::Relaxed);
        WriteResult::Accepted
    }

    /// Discards every block queued so far and returns the consumer to the state "before `PRIME_BLOCKS`
    /// blocks have accumulated" (the model calls this on CRST).
    ///
    /// The consumer applies the flush at its next [AudioConsumer::fill] (within one device period), so
    /// the producer never touches a slot the consumer may be reading. Until then a ring that was full
    /// still rejects new blocks as overrun; with a stopped consumer the flush takes effect when it restarts.
    pub fn flush(&mut self) {
        self.shared.flush_mark.store(self.tail, Ordering::Relaxed);
        self.shared.flush_count.fetch_add(1, Ordering::Release);
        self.shared.flushes.fetch_add(1, Ordering::Relaxed);
    }
}

/// The single consumer of the ring: the CoreAudio render callback or the offline renderer.
pub struct AudioConsumer {
    shared: Arc<Shared>,
    /// Local copy of `shared.head` (only this handle writes it).
    head: u64,
    seen_flush: u64,
    primed: bool,
    /// Block being played out; `pos` is the next sample in it (`BLOCK_SAMPLES` = none left).
    current: AudioBlock,
    pos: usize,
}

impl AudioConsumer {
    /// Fills `out` (interleaved samples; any length, a device period need not be a multiple of a
    /// block) from the ring. Never blocks, allocates or panics. Before the ring is primed, after a flush
    /// and when it runs dry the output is silence: a dry primed ring counts an underrun per missing
    /// block and keeps going; an unprimed one counts `prefill_silent_blocks`.
    pub fn fill(&mut self, out: &mut [i16]) {
        self.apply_flush();
        let mut done = 0;
        while done < out.len() {
            if self.pos == BLOCK_SAMPLES {
                self.load_block();
            }
            let n = (out.len() - done).min(BLOCK_SAMPLES - self.pos);
            out[done..done + n].copy_from_slice(&self.current[self.pos..self.pos + n]);
            self.pos += n;
            done += n;
        }
    }

    fn apply_flush(&mut self) {
        let count = self.shared.flush_count.load(Ordering::Acquire);
        if count == self.seen_flush {
            return;
        }
        self.seen_flush = count;
        let mark = self.shared.flush_mark.load(Ordering::Relaxed);
        if mark > self.head {
            self.head = mark;
            self.shared.head.store(mark, Ordering::Release);
        }
        self.primed = false;
        self.pos = BLOCK_SAMPLES;
    }

    fn load_block(&mut self) {
        let tail = self.shared.tail.load(Ordering::Acquire);
        let queued = tail - self.head;
        if !self.primed && queued >= PRIME_BLOCKS as u64 {
            self.primed = true;
        }
        if self.primed && queued > 0 {
            let slot = &self.shared.slots[(self.head % RING_BLOCKS as u64) as usize];
            // SAFETY: the slot is in the window [head, tail); see the `Sync` impl of `Shared`.
            self.current = unsafe { *slot.get() };
            self.head += 1;
            self.shared.head.store(self.head, Ordering::Release);
            self.shared.blocks_played.fetch_add(1, Ordering::Relaxed);
        } else {
            self.current = [0; BLOCK_SAMPLES];
            let counter = if self.primed {
                &self.shared.underruns
            } else {
                &self.shared.prefill_silent_blocks
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
        self.pos = 0;
    }
}

#[cfg(target_os = "macos")]
pub use coreaudio::{AudioError, DeviceOutput, WavFileOutput};

#[cfg(target_os = "macos")]
mod coreaudio {
    use super::{AudioBlock, AudioConsumer, BLOCK_SAMPLES, CHANNELS, SAMPLE_RATE_HZ};
    use std::ffi::c_void;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;

    type OSStatus = i32;
    type AudioComponent = *mut c_void;
    type AudioUnit = *mut c_void;
    type ExtAudioFileRef = *mut c_void;
    type CFURLRef = *const c_void;

    const FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
    const FORMAT_FLAG_SIGNED_INTEGER: u32 = 1 << 2;
    const FORMAT_FLAG_PACKED: u32 = 1 << 3;
    const UNIT_TYPE_OUTPUT: u32 = u32::from_be_bytes(*b"auou");
    const UNIT_SUBTYPE_DEFAULT_OUTPUT: u32 = u32::from_be_bytes(*b"def ");
    const MANUFACTURER_APPLE: u32 = u32::from_be_bytes(*b"appl");
    const PROPERTY_STREAM_FORMAT: u32 = 8;
    const PROPERTY_SET_RENDER_CALLBACK: u32 = 23;
    const SCOPE_INPUT: u32 = 1;
    const FILE_TYPE_WAVE: u32 = u32::from_be_bytes(*b"WAVE");
    const FILE_FLAG_ERASE_FILE: u32 = 1;
    const PARAM_ERR: OSStatus = -50;

    #[repr(C)]
    struct AudioStreamBasicDescription {
        sample_rate: f64,
        format_id: u32,
        format_flags: u32,
        bytes_per_packet: u32,
        frames_per_packet: u32,
        bytes_per_frame: u32,
        channels_per_frame: u32,
        bits_per_channel: u32,
        reserved: u32,
    }

    #[repr(C)]
    struct AudioComponentDescription {
        component_type: u32,
        component_sub_type: u32,
        component_manufacturer: u32,
        component_flags: u32,
        component_flags_mask: u32,
    }

    #[repr(C)]
    struct AudioBuffer {
        number_channels: u32,
        data_byte_size: u32,
        data: *mut c_void,
    }

    /// `AudioBufferList` with the one buffer this module ever describes; a callback may be handed a
    /// longer list, so it reads only the first buffer through a pointer to this prefix.
    #[repr(C)]
    struct AudioBufferList {
        number_buffers: u32,
        buffers: [AudioBuffer; 1],
    }

    type RenderCallback = extern "C" fn(
        *mut c_void,
        *mut u32,
        *const c_void,
        u32,
        u32,
        *mut AudioBufferList,
    ) -> OSStatus;

    #[repr(C)]
    struct RenderCallbackStruct {
        input_proc: RenderCallback,
        input_proc_ref_con: *mut c_void,
    }

    #[link(name = "AudioToolbox", kind = "framework")]
    unsafe extern "C" {
        fn AudioComponentFindNext(
            component: AudioComponent,
            description: *const AudioComponentDescription,
        ) -> AudioComponent;
        fn AudioComponentInstanceNew(component: AudioComponent, unit: *mut AudioUnit) -> OSStatus;
        fn AudioComponentInstanceDispose(unit: AudioUnit) -> OSStatus;
        fn AudioUnitSetProperty(
            unit: AudioUnit,
            property: u32,
            scope: u32,
            element: u32,
            data: *const c_void,
            size: u32,
        ) -> OSStatus;
        fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
        fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
        fn AudioOutputUnitStart(unit: AudioUnit) -> OSStatus;
        fn AudioOutputUnitStop(unit: AudioUnit) -> OSStatus;
        fn ExtAudioFileCreateWithURL(
            url: CFURLRef,
            file_type: u32,
            format: *const AudioStreamBasicDescription,
            channel_layout: *const c_void,
            flags: u32,
            file: *mut ExtAudioFileRef,
        ) -> OSStatus;
        fn ExtAudioFileWrite(
            file: ExtAudioFileRef,
            frames: u32,
            data: *const AudioBufferList,
        ) -> OSStatus;
        fn ExtAudioFileDispose(file: ExtAudioFileRef) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: *const c_void,
            buffer: *const u8,
            length: isize,
            is_directory: u8,
        ) -> CFURLRef;
        fn CFRelease(object: *const c_void);
    }

    /// A failed system call: its name and `OSStatus` (0 when the call returned no status).
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct AudioError {
        pub call: &'static str,
        pub status: i32,
    }

    impl std::fmt::Display for AudioError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} failed (OSStatus {})", self.call, self.status)
        }
    }

    impl std::error::Error for AudioError {}

    fn check(call: &'static str, status: OSStatus) -> Result<(), AudioError> {
        if status == 0 {
            Ok(())
        } else {
            Err(AudioError { call, status })
        }
    }

    /// 48 kHz, 16-bit, interleaved stereo, signed little-endian: the only format of the contract.
    fn stream_format() -> AudioStreamBasicDescription {
        let bytes_per_frame = (CHANNELS * 2) as u32;
        AudioStreamBasicDescription {
            sample_rate: f64::from(SAMPLE_RATE_HZ),
            format_id: FORMAT_LINEAR_PCM,
            format_flags: FORMAT_FLAG_SIGNED_INTEGER | FORMAT_FLAG_PACKED,
            bytes_per_packet: bytes_per_frame,
            frames_per_packet: 1,
            bytes_per_frame,
            channels_per_frame: CHANNELS as u32,
            bits_per_channel: 16,
            reserved: 0,
        }
    }

    /// Output through the system default output unit (the unit converts 48 kHz/16-bit to the device).
    /// Dropping it stops the unit and frees the consumer.
    pub struct DeviceOutput {
        unit: AudioUnit,
        consumer: *mut AudioConsumer,
        initialized: bool,
        running: bool,
    }

    /// The render callback runs on the system's real-time thread: it only drives
    /// [AudioConsumer::fill] (no allocation, no lock, no Swift/Objective-C).
    extern "C" fn render_callback(
        ref_con: *mut c_void,
        _flags: *mut u32,
        _timestamp: *const c_void,
        _bus: u32,
        _frames: u32,
        io_data: *mut AudioBufferList,
    ) -> OSStatus {
        if ref_con.is_null() || io_data.is_null() {
            return PARAM_ERR;
        }
        // SAFETY: `io_data` is the list CoreAudio hands to the callback; the module asked for one
        // interleaved buffer, and the first buffer is read through the prefix layout.
        let (count, buffer) = unsafe {
            (
                (*io_data).number_buffers,
                ptr::addr_of_mut!((*io_data).buffers[0]),
            )
        };
        // SAFETY: `buffer` points into that list and is valid for the call.
        let (data, bytes) = unsafe { ((*buffer).data, (*buffer).data_byte_size as usize) };
        if count < 1 || data.is_null() || !(data as usize).is_multiple_of(align_of::<i16>()) {
            return PARAM_ERR;
        }
        // SAFETY: `ref_con` is the `AudioConsumer` that `DeviceOutput` owns and frees only after the
        // unit has been stopped and disposed, so no callback runs after the free; `data` is a writable,
        // aligned buffer of `bytes` bytes owned by CoreAudio for this call.
        unsafe {
            let consumer = &mut *(ref_con as *mut AudioConsumer);
            consumer.fill(std::slice::from_raw_parts_mut(data as *mut i16, bytes / 2));
        }
        0
    }

    impl DeviceOutput {
        /// Opens the default output device and starts pulling `consumer`. Silence until the producer
        /// has queued `PRIME_BLOCKS` blocks.
        pub fn start(consumer: AudioConsumer) -> Result<Self, AudioError> {
            let mut output = Self {
                unit: ptr::null_mut(),
                consumer: Box::into_raw(Box::new(consumer)),
                initialized: false,
                running: false,
            };
            let description = AudioComponentDescription {
                component_type: UNIT_TYPE_OUTPUT,
                component_sub_type: UNIT_SUBTYPE_DEFAULT_OUTPUT,
                component_manufacturer: MANUFACTURER_APPLE,
                component_flags: 0,
                component_flags_mask: 0,
            };
            // SAFETY: plain system calls with valid pointers to locals that outlive them; the callback's
            // ref-con stays valid until `Drop` (see `render_callback`). `output` cleans up on every early return.
            unsafe {
                let component = AudioComponentFindNext(ptr::null_mut(), &description);
                if component.is_null() {
                    return Err(AudioError {
                        call: "AudioComponentFindNext",
                        status: 0,
                    });
                }
                check(
                    "AudioComponentInstanceNew",
                    AudioComponentInstanceNew(component, &mut output.unit),
                )?;
                let format = stream_format();
                check(
                    "AudioUnitSetProperty(StreamFormat)",
                    AudioUnitSetProperty(
                        output.unit,
                        PROPERTY_STREAM_FORMAT,
                        SCOPE_INPUT,
                        0,
                        ptr::from_ref(&format).cast(),
                        size_of::<AudioStreamBasicDescription>() as u32,
                    ),
                )?;
                let callback = RenderCallbackStruct {
                    input_proc: render_callback,
                    input_proc_ref_con: output.consumer.cast(),
                };
                check(
                    "AudioUnitSetProperty(SetRenderCallback)",
                    AudioUnitSetProperty(
                        output.unit,
                        PROPERTY_SET_RENDER_CALLBACK,
                        SCOPE_INPUT,
                        0,
                        ptr::from_ref(&callback).cast(),
                        size_of::<RenderCallbackStruct>() as u32,
                    ),
                )?;
                check("AudioUnitInitialize", AudioUnitInitialize(output.unit))?;
                output.initialized = true;
                check("AudioOutputUnitStart", AudioOutputUnitStart(output.unit))?;
                output.running = true;
            }
            Ok(output)
        }
    }

    impl Drop for DeviceOutput {
        fn drop(&mut self) {
            // SAFETY: the unit is stopped, uninitialized and disposed (each only if it got that far)
            // before the consumer that the callback reads is freed.
            unsafe {
                if self.running {
                    AudioOutputUnitStop(self.unit);
                }
                if self.initialized {
                    AudioUnitUninitialize(self.unit);
                }
                if !self.unit.is_null() {
                    AudioComponentInstanceDispose(self.unit);
                }
                drop(Box::from_raw(self.consumer));
            }
        }
    }

    /// Offline output: the same [AudioConsumer] the device would pull, driven by the caller block by
    /// block into a 48 kHz/16-bit/stereo WAV file written through ExtAudioFile. No sound device is used.
    pub struct WavFileOutput {
        file: ExtAudioFileRef,
        consumer: AudioConsumer,
        frames_written: u64,
    }

    impl WavFileOutput {
        /// Creates (or truncates) the file at `path`.
        pub fn create(path: &Path, consumer: AudioConsumer) -> Result<Self, AudioError> {
            let bytes = path.as_os_str().as_bytes();
            let format = stream_format();
            let mut file: ExtAudioFileRef = ptr::null_mut();
            // SAFETY: `bytes` is valid for `len` bytes; the URL is released after the call, the file
            // reference is owned by the returned value.
            let status = unsafe {
                let url = CFURLCreateFromFileSystemRepresentation(
                    ptr::null(),
                    bytes.as_ptr(),
                    bytes.len() as isize,
                    0,
                );
                if url.is_null() {
                    return Err(AudioError {
                        call: "CFURLCreateFromFileSystemRepresentation",
                        status: 0,
                    });
                }
                let status = ExtAudioFileCreateWithURL(
                    url,
                    FILE_TYPE_WAVE,
                    &format,
                    ptr::null(),
                    FILE_FLAG_ERASE_FILE,
                    &mut file,
                );
                CFRelease(url);
                status
            };
            check("ExtAudioFileCreateWithURL", status)?;
            Ok(Self {
                file,
                consumer,
                frames_written: 0,
            })
        }

        /// Pulls `blocks` blocks from the ring (silence where it is unprimed or dry, with the
        /// usual counters) and appends them to the file.
        pub fn render_blocks(&mut self, blocks: usize) -> Result<(), AudioError> {
            for _ in 0..blocks {
                let mut block: AudioBlock = [0; BLOCK_SAMPLES];
                self.consumer.fill(&mut block);
                let list = AudioBufferList {
                    number_buffers: 1,
                    buffers: [AudioBuffer {
                        number_channels: CHANNELS as u32,
                        data_byte_size: size_of::<AudioBlock>() as u32,
                        data: block.as_mut_ptr().cast(),
                    }],
                };
                // SAFETY: `list` describes `block`, which outlives the call; the file is open.
                let status =
                    unsafe { ExtAudioFileWrite(self.file, super::BLOCK_FRAMES as u32, &list) };
                check("ExtAudioFileWrite", status)?;
                self.frames_written += super::BLOCK_FRAMES as u64;
            }
            Ok(())
        }

        /// Closes the file (this finalizes the WAV header) and returns the number of frames written.
        pub fn finish(mut self) -> Result<u64, AudioError> {
            let file = std::mem::replace(&mut self.file, ptr::null_mut());
            // SAFETY: `file` is open and is disposed exactly once (Drop sees a null reference).
            check("ExtAudioFileDispose", unsafe { ExtAudioFileDispose(file) })?;
            Ok(self.frames_written)
        }
    }

    impl Drop for WavFileOutput {
        fn drop(&mut self) {
            if !self.file.is_null() {
                // SAFETY: the file is open and disposed once; an error cannot be reported from Drop.
                unsafe { ExtAudioFileDispose(self.file) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block whose every sample equals `tag`.
    fn tagged(tag: i16) -> AudioBlock {
        [tag; BLOCK_SAMPLES]
    }

    fn pull_block(consumer: &mut AudioConsumer) -> AudioBlock {
        let mut block = [0; BLOCK_SAMPLES];
        consumer.fill(&mut block);
        block
    }

    #[test]
    fn plays_blocks_in_order_only_after_priming() {
        let AudioRing {
            mut producer,
            mut consumer,
            stats,
        } = audio_ring();
        for tag in 1..PRIME_BLOCKS as i16 {
            assert_eq!(producer.write_block(&tagged(tag)), WriteResult::Accepted);
        }
        // Three queued blocks are not enough: silence, counted as prefill and not as underrun.
        assert_eq!(pull_block(&mut consumer), [0; BLOCK_SAMPLES]);
        let s = stats.snapshot();
        assert_eq!(
            (s.prefill_silent_blocks, s.underruns, s.blocks_played),
            (1, 0, 0)
        );
        producer.write_block(&tagged(PRIME_BLOCKS as i16));
        for tag in 1..=PRIME_BLOCKS as i16 {
            assert_eq!(pull_block(&mut consumer), tagged(tag));
        }
        let s = stats.snapshot();
        assert_eq!((s.blocks_written, s.blocks_played, s.underruns), (4, 4, 0));
    }

    #[test]
    fn full_ring_drops_new_blocks_and_counts_them() {
        let AudioRing {
            mut producer,
            mut consumer,
            stats,
        } = audio_ring();
        for tag in 0..RING_BLOCKS as i16 {
            assert_eq!(
                producer.write_block(&tagged(tag + 1)),
                WriteResult::Accepted
            );
        }
        assert_eq!(producer.write_block(&tagged(99)), WriteResult::Dropped);
        assert_eq!(stats.snapshot().dropped_blocks, 1);
        // The oldest blocks are still the ones played; the dropped one never appears.
        for tag in 0..RING_BLOCKS as i16 {
            assert_eq!(pull_block(&mut consumer), tagged(tag + 1));
        }
        assert_eq!(producer.write_block(&tagged(100)), WriteResult::Accepted);
    }

    #[test]
    fn dry_ring_plays_silence_counts_underruns_and_keeps_going() {
        let AudioRing {
            mut producer,
            mut consumer,
            stats,
        } = audio_ring();
        for tag in 1..=PRIME_BLOCKS as i16 {
            producer.write_block(&tagged(tag));
        }
        for _ in 0..PRIME_BLOCKS {
            pull_block(&mut consumer);
        }
        assert_eq!(pull_block(&mut consumer), [0; BLOCK_SAMPLES]);
        assert_eq!(pull_block(&mut consumer), [0; BLOCK_SAMPLES]);
        assert_eq!(stats.snapshot().underruns, 2);
        // The stream is not stopped and not re-primed: one new block is played at once.
        producer.write_block(&tagged(7));
        assert_eq!(pull_block(&mut consumer), tagged(7));
        assert_eq!(stats.snapshot().underruns, 2);
    }

    #[test]
    fn flush_discards_queued_blocks_and_primes_again() {
        let AudioRing {
            mut producer,
            mut consumer,
            stats,
        } = audio_ring();
        for tag in 1..=6 {
            producer.write_block(&tagged(tag));
        }
        assert_eq!(pull_block(&mut consumer), tagged(1));
        producer.flush();
        // Blocks written after the flush survive it; fewer than PRIME_BLOCKS of them are not played.
        for tag in 101..=103 {
            producer.write_block(&tagged(tag));
        }
        assert_eq!(pull_block(&mut consumer), [0; BLOCK_SAMPLES]);
        producer.write_block(&tagged(104));
        for tag in 101..=104 {
            assert_eq!(pull_block(&mut consumer), tagged(tag));
        }
        let s = stats.snapshot();
        assert_eq!((s.flushes, s.underruns, s.blocks_played), (1, 0, 5));
    }

    #[test]
    fn flush_discards_the_unplayed_rest_of_the_current_block() {
        let AudioRing {
            mut producer,
            mut consumer,
            ..
        } = audio_ring();
        for tag in 1..=PRIME_BLOCKS as i16 {
            producer.write_block(&tagged(tag));
        }
        let mut half = [0; BLOCK_SAMPLES / 2];
        consumer.fill(&mut half);
        assert_eq!(half, [1; BLOCK_SAMPLES / 2]);
        producer.flush();
        consumer.fill(&mut half);
        assert_eq!(half, [0; BLOCK_SAMPLES / 2]);
    }

    #[test]
    fn flush_of_a_full_ring_frees_it_at_the_next_pull() {
        let AudioRing {
            mut producer,
            mut consumer,
            ..
        } = audio_ring();
        for tag in 0..RING_BLOCKS as i16 {
            producer.write_block(&tagged(tag + 1));
        }
        producer.flush();
        // The consumer has not applied the flush yet, so the ring still counts as full.
        assert_eq!(producer.write_block(&tagged(50)), WriteResult::Dropped);
        assert_eq!(pull_block(&mut consumer), [0; BLOCK_SAMPLES]);
        assert_eq!(producer.write_block(&tagged(51)), WriteResult::Accepted);
    }

    #[test]
    fn device_periods_that_are_not_a_block_multiple_keep_the_stream_continuous() {
        let AudioRing {
            mut producer,
            mut consumer,
            stats,
        } = audio_ring();
        // Sample n of the stream holds n modulo 2^15.
        let mut next = 0usize;
        for _ in 0..8 {
            let mut block = [0i16; BLOCK_SAMPLES];
            for sample in &mut block {
                *sample = (next % 32768) as i16;
                next += 1;
            }
            producer.write_block(&block);
        }
        let mut out = vec![0i16; 2 * 512]; // 512-frame periods, as a device may ask
        let mut expected = 0usize;
        while expected + out.len() <= 8 * BLOCK_SAMPLES {
            consumer.fill(&mut out);
            for sample in &out {
                assert_eq!(*sample, (expected % 32768) as i16);
                expected += 1;
            }
        }
        assert_eq!(stats.snapshot().underruns, 0);
    }

    #[test]
    fn concurrent_producer_and_consumer_never_tear_or_reorder_blocks() {
        const TOTAL: i16 = 3000;
        let AudioRing {
            mut producer,
            mut consumer,
            stats,
        } = audio_ring();
        let writer = std::thread::spawn(move || {
            for tag in 1..=TOTAL {
                while producer.write_block(&tagged(tag)) == WriteResult::Dropped {
                    std::thread::yield_now();
                }
            }
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut last = 0;
        while last < TOTAL {
            assert!(std::time::Instant::now() < deadline, "consumer starved");
            let block = pull_block(&mut consumer);
            if block[0] == 0 {
                assert!(block.iter().all(|s| *s == 0), "torn silence block");
                std::thread::yield_now();
                continue;
            }
            assert!(block.iter().all(|s| *s == block[0]), "torn block");
            assert_eq!(block[0], last + 1, "blocks out of order or lost");
            last = block[0];
        }
        writer.join().unwrap();
        assert_eq!(stats.snapshot().blocks_played, TOTAL as u64);
    }

    /// Rising zero crossings of channel 0, with linear interpolation, over the whole signal.
    #[cfg(target_os = "macos")]
    fn measured_frequency_hz(left: &[i16]) -> f64 {
        let mut crossings = Vec::new();
        for i in 1..left.len() {
            let (a, b) = (f64::from(left[i - 1]), f64::from(left[i]));
            if a <= 0.0 && b > 0.0 {
                crossings.push((i - 1) as f64 + (-a) / (b - a));
            }
        }
        assert!(crossings.len() > 2, "no tone");
        let periods = (crossings.len() - 1) as f64;
        periods * f64::from(SAMPLE_RATE_HZ) / (crossings[crossings.len() - 1] - crossings[0])
    }

    #[cfg(target_os = "macos")]
    mod offline {
        use super::*;
        use crate::audio_output::{DeviceOutput, WavFileOutput};
        use std::path::PathBuf;
        use std::time::Duration;

        struct Wav {
            format: u16,
            channels: u16,
            rate: u32,
            bits: u16,
            samples: Vec<i16>,
        }

        /// Minimal independent RIFF/WAVE reader (chunk walk), so the check does not trust the writer.
        fn read_wav(path: &std::path::Path) -> Wav {
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(&bytes[0..4], b"RIFF");
            assert_eq!(&bytes[8..12], b"WAVE");
            let mut wav = Wav {
                format: 0,
                channels: 0,
                rate: 0,
                bits: 0,
                samples: Vec::new(),
            };
            let mut at = 12;
            while at + 8 <= bytes.len() {
                let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
                let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
                match &bytes[at..at + 4] {
                    b"fmt " => {
                        wav.format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                        wav.channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                        wav.rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                        wav.bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
                    }
                    b"data" => {
                        assert_eq!(body.len(), size, "data chunk truncated");
                        wav.samples = body
                            .chunks_exact(2)
                            .map(|b| i16::from_le_bytes([b[0], b[1]]))
                            .collect();
                    }
                    _ => {}
                }
                at += 8 + size + (size & 1);
            }
            wav
        }

        fn temp_wav(name: &str) -> PathBuf {
            std::env::temp_dir().join(format!("od-host-{name}-{}.wav", std::process::id()))
        }

        /// Synthetic tone: sample n = round(amplitude * sin(2 pi f n / 48000)) on both channels.
        fn tone_block(first_frame: usize, hz: f64, amplitude: f64) -> AudioBlock {
            let mut block = [0; BLOCK_SAMPLES];
            for frame in 0..BLOCK_FRAMES {
                let phase = 2.0 * std::f64::consts::PI * hz * (first_frame + frame) as f64
                    / f64::from(SAMPLE_RATE_HZ);
                let value = (amplitude * phase.sin()).round() as i16;
                block[2 * frame] = value;
                block[2 * frame + 1] = value;
            }
            block
        }

        fn render_tone(name: &str, hz: f64, amplitude: f64, seconds: usize) -> (Wav, AudioStats) {
            let blocks = seconds * 100;
            let AudioRing {
                mut producer,
                consumer,
                stats,
            } = audio_ring();
            let path = temp_wav(name);
            let mut out = WavFileOutput::create(&path, consumer).unwrap();
            for index in 0..blocks {
                let block = tone_block(index * BLOCK_FRAMES, hz, amplitude);
                assert_eq!(producer.write_block(&block), WriteResult::Accepted);
                if index + 1 >= PRIME_BLOCKS {
                    out.render_blocks(1).unwrap();
                }
            }
            out.render_blocks(PRIME_BLOCKS - 1).unwrap();
            assert_eq!(out.finish().unwrap(), (blocks * BLOCK_FRAMES) as u64);
            let wav = read_wav(&path);
            std::fs::remove_file(&path).unwrap();
            (wav, stats.snapshot())
        }

        /// Tolerances (recorded in AUDIO_DEVICE.md section 10): frequency 0.1 Hz (the crossing count
        /// over one second resolves to far less; the path is integer-exact), amplitude peak 1 LSB
        /// (rounding of the generator) and RMS 0.5 percent.
        #[test]
        fn offline_wav_carries_the_synthetic_tone_unchanged() {
            for hz in [440.0, 1000.0] {
                let amplitude = 16_000.0;
                let (wav, stats) = render_tone(&format!("tone{hz}"), hz, amplitude, 1);
                assert_eq!(
                    (wav.format, wav.channels, wav.rate, wav.bits),
                    (1, 2, 48_000, 16)
                );
                assert_eq!(wav.samples.len(), 2 * 48_000);
                let left: Vec<i16> = wav.samples.iter().step_by(2).copied().collect();
                let right: Vec<i16> = wav.samples.iter().skip(1).step_by(2).copied().collect();
                assert_eq!(left, right);
                let measured = measured_frequency_hz(&left);
                assert!(
                    (measured - hz).abs() < 0.1,
                    "{hz} Hz measured as {measured}"
                );
                let peak = left.iter().map(|s| i32::from(*s).abs()).max().unwrap();
                assert!((peak - amplitude as i32).abs() <= 1, "peak {peak}");
                let rms = (left.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
                    / left.len() as f64)
                    .sqrt();
                let expected_rms = amplitude / std::f64::consts::SQRT_2;
                assert!(
                    (rms - expected_rms).abs() / expected_rms < 0.005,
                    "rms {rms}"
                );
                // Bit exact against the generator: nothing between the ring and the file alters samples.
                let expected: Vec<i16> = (0..100)
                    .flat_map(|index| tone_block(index * BLOCK_FRAMES, hz, amplitude))
                    .collect();
                assert_eq!(wav.samples, expected);
                assert_eq!(
                    (stats.underruns, stats.dropped_blocks, stats.blocks_played),
                    (0, 0, 100)
                );
            }
        }

        #[test]
        fn offline_underrun_writes_silence_and_counts_it() {
            let AudioRing {
                mut producer,
                consumer,
                stats,
            } = audio_ring();
            let path = temp_wav("underrun");
            let mut out = WavFileOutput::create(&path, consumer).unwrap();
            for index in 0..PRIME_BLOCKS {
                producer.write_block(&tone_block(index * BLOCK_FRAMES, 1000.0, 10_000.0));
            }
            out.render_blocks(PRIME_BLOCKS + 6).unwrap();
            assert_eq!(
                out.finish().unwrap(),
                ((PRIME_BLOCKS + 6) * BLOCK_FRAMES) as u64
            );
            let wav = read_wav(&path);
            std::fs::remove_file(&path).unwrap();
            let played = PRIME_BLOCKS * BLOCK_SAMPLES;
            assert_eq!(wav.samples.len(), played + 6 * BLOCK_SAMPLES);
            assert!(wav.samples[..played].iter().any(|s| *s != 0));
            assert!(wav.samples[played..].iter().all(|s| *s == 0));
            assert_eq!(stats.snapshot().underruns, 6);
        }

        /// Starts the real default output with an empty ring: it only outputs silence, so it is safe
        /// to run; it needs an audio device, hence manual: `cargo test -p od-host --lib audio_output -- --ignored device`.
        #[test]
        #[ignore = "needs a macOS audio output device"]
        fn device_output_starts_pulls_silence_and_stops() {
            let AudioRing {
                consumer, stats, ..
            } = audio_ring();
            let output = DeviceOutput::start(consumer).unwrap();
            std::thread::sleep(Duration::from_millis(500));
            drop(output);
            let s = stats.snapshot();
            // About 50 periods of 10 ms; the device period is not exactly one block, so only a lower bound.
            assert!(s.prefill_silent_blocks >= 10, "{s:?}");
            assert_eq!((s.underruns, s.blocks_played), (0, 0));
        }

        /// Residence time of a block in the ring between `write_block` and the pull that loads it, with a
        /// producer and a consumer each paced at 10 ms (simulation of the device clock, not a device
        /// measurement): manual, prints the numbers.
        #[test]
        #[ignore = "timing measurement: run manually"]
        fn measure_ring_residence_time() {
            use std::sync::atomic::AtomicU64;
            use std::time::Instant;
            const BLOCKS: usize = 300;
            let AudioRing {
                mut producer,
                mut consumer,
                stats,
            } = audio_ring();
            let base = Instant::now();
            let written: Arc<Vec<AtomicU64>> =
                Arc::new((0..BLOCKS).map(|_| AtomicU64::new(0)).collect());
            let written_in_thread = Arc::clone(&written);
            let writer = std::thread::spawn(move || {
                for index in 0..BLOCKS {
                    let due = base + Duration::from_millis(10 * index as u64);
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                    written_in_thread[index]
                        .store(base.elapsed().as_nanos() as u64, Ordering::Release);
                    producer.write_block(&tagged(index as i16 + 1));
                }
            });
            let mut latencies_ms = Vec::new();
            let mut tick = 0u64;
            while latencies_ms.len() < BLOCKS - PRIME_BLOCKS {
                let due = base + Duration::from_millis(10 * tick + 3);
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                tick += 1;
                let block = pull_block(&mut consumer);
                if block[0] != 0 {
                    let at = base.elapsed().as_nanos() as u64;
                    let sent = written[block[0] as usize - 1].load(Ordering::Acquire);
                    latencies_ms.push((at - sent) as f64 / 1e6);
                }
                assert!(tick < 2 * BLOCKS as u64, "stream never completed");
            }
            writer.join().unwrap();
            let mean = latencies_ms.iter().sum::<f64>() / latencies_ms.len() as f64;
            let max = latencies_ms.iter().copied().fold(0.0, f64::max);
            let min = latencies_ms.iter().copied().fold(f64::MAX, f64::min);
            eprintln!(
                "residence ms: mean {mean:.1} min {min:.1} max {max:.1}; {:?}",
                stats.snapshot()
            );
        }
    }
}
