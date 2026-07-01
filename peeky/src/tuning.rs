//! Central registry of tunable knobs. Change a number, recompile, see
//! the effect on latency or correctness. Structural constants (URLs,
//! model IDs, header names) live near their code; only behavior dials
//! live here. All times in milliseconds unless noted.

// ────── audio capture ──────

/// Audio captured before press is buffered in this ring; flushed on
/// press so the first syllable isn't missed.
/// ↑ catches more leading audio (good for fast talkers). costs memory.
/// ↓ less leading audio captured. 0 = nothing buffered before press.
pub const AUDIO_PREROLL_MS: u64 = 0;

/// How long to keep forwarding audio to Deepgram after release.
/// ↑ more reliable last-syllable capture. adds latency.
/// ↓ faster EOS to Deepgram. risks clipping the final word.
pub const AUDIO_POST_RELEASE_GRACE_MS: u64 = 200;

// ────── STT ──────

/// After Deepgram sends a non-empty FINAL, wait this long for any
/// additional FINALs before returning.
/// ↑ catches multi-segment utterances (pauses, "Hello. My name is X").
/// ↓ faster transcript return. risks truncating split utterances.
pub const STT_QUIESCENCE_MS: u64 = 150;

// ────── TTS first-flush ──────

/// Min chars before the eager flush accepts a comma/semicolon/colon
/// as a flush point (instead of waiting for . ! ?).
/// ↑ only flushes on longer opening clauses (smoother prosody).
/// ↓ catches shorter clauses like "Hi there," (faster first audio).
pub const TTS_FIRST_FLUSH_MIN_CHARS: usize = 12;

// ────── routelet classifier ──────

/// Minimum routelet confidence required to accept its prediction on-device.
/// Below this the turn falls back to the Claude classifier for a second opinion.
///
/// Set high on purpose. routelet's max-softmax saturates near 0.98 for almost
/// everything, including garbled/out-of-distribution input, so a low gate never
/// fired (see the deferral analysis in routelet/report). At 0.55 it deferred ~0%
/// of OOD probes; at 0.95 it catches a meaningful share of them while deferring
/// only ~1 to 2% of real in-distribution commands.
/// ↑ defers more turns to Claude (catches more OOD, adds latency and cost).
/// ↓ keeps more turns on-device (faster, but the gate stops catching OOD).
pub const ROUTELET_CONFIDENCE_THRESHOLD: f32 = 0.95;

/// Max distillation samples drained and POSTed in one uploader wakeup.
/// ↑ fewer wakeups under bursty use. larger transient batch if the proxy is slow.
/// ↓ steadier trickle of small requests. more task wakeups.
pub const ROUTELET_UPLOAD_BATCH_MAX: usize = 32;

/// Seconds to retire a proxy-minted STT/TTS token before its real TTL, so a
/// turn never opens a stream with a token that expires mid-flight. The proxy
/// mints 3600s tokens, so the cache effectively lasts a session minus this.
/// ↑ re-mints sooner (safer against clock skew and long turns, more mints).
/// ↓ squeezes more reuse out of each token (fewer mints, tighter expiry race).
pub const PROXY_TOKEN_REFRESH_MARGIN_SECS: u64 = 120;

// ────── Claude agent loop ──────

/// Hard cap on agent loop iterations per turn.
/// ↑ allows longer multi-step plans. risks runaway token burn.
/// ↓ tighter cost ceiling. may truncate legitimate chains.
pub const AGENT_MAX_STEPS: usize = 10;

/// Wait between firing a tool action and capturing the next screenshot.
/// Lets the UI repaint, animations settle.
/// ↑ more reliable screenshots after UI changes. step latency tax.
/// ↓ faster step-to-step. risks capturing pre-animation state.
pub const AGENT_SETTLE_MS: u64 = 600;

/// Max screenshots kept inline in messages history. Older ones get
/// their image bytes stripped.
/// ↑ more visual context across steps. bigger requests.
/// ↓ tighter request bodies. less long-range visual memory.
pub const AGENT_KEEP_RECENT_SCREENSHOTS: usize = 3;

// ────── Claude integration path ──────

/// Max integration tool dispatches per turn before a forced spoken summary.
/// Lets chains like spotlight_search → finder_open complete in one turn.
/// ↑ longer chains finish. each extra call adds a model round-trip (~1s)
///   before speech starts.
/// ↓ snappier turns. multi-tool requests get narrated, not finished.
pub const INTEGRATION_MAX_TOOL_CALLS: usize = 3;

// ────── Working context (Tier 0) ──────

/// Recent voice turns kept verbatim in the live working context and replayed
/// into chat/agent requests.
/// ↑ better in-conversation recall. more tokens + latency per turn.
/// ↓ leaner requests. shorter conversational memory.
pub const WORKING_CONTEXT_RECENT_TURNS: usize = 6;

/// Turn count that triggers compaction: once `recent` exceeds this, the turns
/// older than RECENT_TURNS are folded into the running summary off the hot
/// path. Kept above RECENT_TURNS for hysteresis, so it does not compact every
/// turn.
/// ↑ compact less often. larger peak request before it kicks in.
/// ↓ compact sooner. more frequent summarizer calls.
pub const WORKING_CONTEXT_COMPACT_AT: usize = 10;

// ────── portal screen capture (GNOME/Wayland fallback) ──────

/// How long to wait for the PipeWire stream to report its first negotiated
/// video format after starting a portal ScreenCast session, before giving up.
/// ↑ tolerates a slow first-time consent dialog / compositor. slower failure signal.
/// ↓ fails fast on a genuinely broken portal. may abandon a legitimately slow first run.
pub const PORTAL_STREAM_READY_TIMEOUT_MS: u64 = 5_000;

/// Max age of a cached PipeWire frame `capture_resized_for_claude` will reuse
/// without waiting for a fresher one.
/// ↑ fewer waits on a slow-to-emit stream. risks a stale screenshot.
/// ↓ always near-live. more waiting when frames arrive slower than voice turns.
pub const PORTAL_FRAME_MAX_AGE_MS: u64 = 250;

// ────── evdev mouse position (GNOME/Wayland fallback) ──────

/// Interval between drift-correcting X11 resyncs of the evdev-integrated
/// mouse position. XQueryPointer only reflects live motion while the cursor
/// is over an XWayland-backed window, so this can't just replace the evdev
/// reading outright — it's a periodic nudge, not a source of truth.
/// ↑ less frequent correction. more accumulated drift between resyncs.
/// ↓ tighter drift bound. more X11 queries (cheap, but still periodic wakeups).
pub const MOUSE_EVDEV_RESYNC_MS: u64 = 2_000;

/// Fraction of the gap to the X11 reading closed on each resync tick, so a
/// stale/frozen X11 value (cursor currently over a native Wayland surface)
/// can't yank the overlay to the wrong spot.
/// ↑ corrects drift faster. more visible nudging if the X11 reading is stale.
/// ↓ smoother, slower correction. drift persists longer between good resyncs.
pub const MOUSE_EVDEV_RESYNC_BLEND: f64 = 0.2;

// ────── Alt region analysis ──────

/// Crop size (pixels) centered on the mouse when Alt triggers a region
/// analysis.
/// ↑ more context for Claude to describe. slower upload, costlier call.
/// ↓ tighter, faster analysis. may miss context just outside the crop.
pub const ANALYZE_REGION_WIDTH: u32 = 480;
pub const ANALYZE_REGION_HEIGHT: u32 = 360;

/// How long the on-screen description bubble stays before auto-dismissing.
/// Mirrors `ai_cursor::common::POINT_DURATION`'s revert-after pattern.
/// ↑ more time to read. lingers over what you're pointing at next.
/// ↓ snappier turnover. may vanish before a longer description is read.
pub const DESCRIBE_BUBBLE_DURATION_MS: u64 = 6_000;

/// Max characters per line before the description bubble wraps.
/// ↑ wider bubble, fewer lines. ↓ narrower bubble, more lines.
pub const DESCRIBE_BUBBLE_MAX_CHARS_PER_LINE: usize = 40;
