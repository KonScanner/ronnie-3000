use thiserror::Error;

/// Error returned while reading or validating a NAM document.
#[derive(Debug, Error)]
pub enum NamError {
    /// Input exceeds the 50 MiB document limit.
    #[error("NAM input exceeds the 50 MiB limit")]
    InputTooLarge,
    /// JSON nesting exceeds depth 64.
    #[error("NAM JSON nesting exceeds depth 64")]
    NestingTooDeep,
    /// JSON syntax, UTF-8, or document field validation failed.
    #[error("invalid NAM JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Version text does not use numeric `major.minor.patch` syntax.
    #[error("invalid NAM version `{0}`")]
    InvalidVersion(String),
    /// Version falls outside the reference-supported range.
    #[error("unsupported NAM version `{0}`")]
    UnsupportedVersion(String),
    /// Architecture is not supported by this parser.
    #[error("unsupported NAM architecture `{0}`")]
    UnsupportedArchitecture(String),
}

/// Error returned while compiling, preparing, or processing a model.
#[derive(Debug, Error)]
pub enum DspError {
    /// Document does not match the strict A2 architecture.
    #[error("document does not match the A2 architecture")]
    InvalidA2Shape,
    /// Weight stream length differs from the compiled architecture.
    #[error("expected {expected} model weights, got {actual}")]
    WeightCount {
        /// Required weight count.
        expected: usize,
        /// Supplied weight count.
        actual: usize,
    },
    /// Sample rate is not finite and positive.
    #[error("sample rate must be finite and positive")]
    InvalidSampleRate,
    /// Requested buffer layout cannot be represented or allocated safely.
    #[error("invalid or unsupported buffer size")]
    InvalidBufferSize,
    /// Engine channel specialization differs from the compiled model.
    #[error("expected {expected} model channels, got {actual}")]
    ChannelMismatch {
        /// Engine channel count.
        expected: usize,
        /// Compiled model channel count.
        actual: usize,
    },
    /// Processing was requested before preparation.
    #[error("model engine is not prepared")]
    NotPrepared,
    /// Input and output lengths differ.
    #[error("input length {input} differs from output length {output}")]
    InvalidBufferLength {
        /// Input frame count.
        input: usize,
        /// Output frame count.
        output: usize,
    },
    /// Callback exceeds the prepared maximum.
    #[error("callback has {actual} frames, exceeding prepared maximum {max}")]
    BlockTooLarge {
        /// Prepared maximum frame count.
        max: usize,
        /// Supplied frame count.
        actual: usize,
    },
    /// Container structure, tiers, or child architecture is invalid.
    #[error("invalid slimmable container")]
    InvalidContainer,
    /// Known child and container sample rates differ.
    #[error("expected sample rate {expected}, got {actual}")]
    SampleRateMismatch {
        /// Container sample rate.
        expected: f64,
        /// Child sample rate.
        actual: f64,
    },
    /// JSON serialization failed while decoding a child document.
    #[error("could not encode container child: {0}")]
    Json(#[from] serde_json::Error),
    /// A complete child document could not be parsed.
    #[error("could not parse container child: {0}")]
    Nam(#[from] NamError),
    /// Oversampling factor is not 1, 2, 4, or 8.
    #[error("oversampling factor must be 1, 2, 4, or 8")]
    InvalidOversamplingFactor,
    /// Voice count is not 1 or 2.
    #[error("voice count must be 1 or 2")]
    InvalidVoiceCount,
    /// Planar channel count is not 1 or 2.
    #[error("planar channel count must be 1 or 2")]
    InvalidChannelCount,
    /// Dual mono requires two channels and two voices.
    #[error("dual mono requires two channels and two voices")]
    InvalidVoiceConfiguration,
    /// Planar buffer length is not divisible by its channel count.
    #[error("planar buffer length is not divisible by its channel count")]
    InvalidPlanarLength,
    /// Benchmark configuration or timing samples are invalid.
    #[error("invalid benchmark configuration or timing samples")]
    InvalidBenchmarkInput,
    /// Bytes do not match the content digest and length in a compiled-cache key.
    #[error("model bytes do not match the compiled-cache content key")]
    ContentMismatch,
    /// A compiled-model cache cannot store any entries.
    #[error("compiled-model cache has zero capacity")]
    CacheCapacity,
    /// A worker job failed while processing a model phase or voice.
    #[error("model worker processing failed")]
    ProcessingFailed,
}

/// Error returned while parsing or normalizing tone metadata.
#[derive(Debug, Error)]
pub enum ToneError {
    /// Tone JSON exceeds the bounded input size.
    #[error("tone JSON exceeds the 50 MiB limit")]
    InputTooLarge,
    /// JSON syntax or serialization failed.
    #[error("invalid tone JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The parsed JSON does not have the required document shape.
    #[error("invalid tone document: {0}")]
    InvalidDocument(&'static str),
    /// A model URL could not be parsed.
    #[error("invalid model URL: {0}")]
    Url(#[from] url::ParseError),
    /// The tone exceeds the Reference model-count bound.
    #[error("tone has {actual} models, exceeding the {max}-model limit")]
    TooManyModels {
        /// Supplied model count.
        actual: usize,
        /// Maximum model count.
        max: usize,
    },
    /// Catalog persistence cannot retain an absent active model.
    #[error("active model {0} is not present in the catalog tone")]
    MissingActiveModel(crate::ModelId),
}

/// Error returned while parsing chain or session snapshots.
#[derive(Debug, Error)]
pub enum ChainError {
    /// Tone parsing failed.
    #[error(transparent)]
    Tone(#[from] ToneError),
    /// The parsed value does not have the required document shape.
    #[error("invalid chain document: {0}")]
    InvalidDocument(&'static str),
    /// A lane exceeds the block-count bound.
    #[error("chain lane has {actual} blocks, exceeding the {max}-block limit")]
    TooManyBlocks {
        /// Supplied block count.
        actual: usize,
        /// Maximum block count.
        max: usize,
    },
    /// EQ data exceeds the six-band bound.
    #[error("EQ has {actual} bands, exceeding the {max}-band limit")]
    TooManyEqBands {
        /// Supplied band count.
        actual: usize,
        /// Maximum band count.
        max: usize,
    },
    /// Session MIDI data exceeds the bounded mapping count.
    #[error("session has {actual} MIDI mappings, exceeding the {max}-mapping limit")]
    TooManyMidiMappings {
        /// Supplied mapping count.
        actual: usize,
        /// Maximum mapping count.
        max: usize,
    },
}

/// Error returned while decoding a bounded WAV document.
#[derive(Debug, Error)]
pub enum AudioError {
    /// WAV input exceeds the local file limit.
    #[error("WAV input exceeds the 50 MiB limit")]
    InputTooLarge,
    /// RIFF/WAVE framing is absent or invalid.
    #[error("invalid RIFF/WAVE header")]
    InvalidHeader,
    /// A declared chunk or field is truncated.
    #[error("truncated WAV input")]
    Truncated,
    /// The `fmt ` chunk is missing.
    #[error("WAV format chunk is missing")]
    MissingFormat,
    /// The encoding is not supported by the Reference local loader.
    #[error("unsupported WAV format tag {0}")]
    UnsupportedFormat(u16),
    /// Channel count is zero or exceeds the bounded maximum.
    #[error("invalid WAV channel count")]
    InvalidChannelCount,
    /// Sample rate is zero.
    #[error("invalid WAV sample rate")]
    InvalidSampleRate,
    /// Data and block-alignment declarations disagree.
    #[error("invalid WAV data size")]
    InvalidDataSize,
    /// Declared counts or sizes overflow a safe allocation.
    #[error("WAV allocation size is invalid or too large")]
    SizeOverflow,
    /// No complete audio frames are present.
    #[error("WAV contains no audio frames")]
    EmptyAudio,
}

/// Error returned during local discovery, validation, or tone synthesis.
#[derive(Debug, Error)]
pub enum LocalError {
    /// A local file exceeds 50 MiB.
    #[error("local file exceeds the 50 MiB limit")]
    InputTooLarge,
    /// More than 300 models or too many directory entries were supplied.
    #[error("local input has {actual} files, exceeding the {max}-file limit")]
    TooManyFiles {
        /// Supplied count.
        actual: usize,
        /// Bounded maximum.
        max: usize,
    },
    /// The filename is not `.nam` or `.wav`.
    #[error("only .nam and .wav files are supported")]
    UnsupportedExtension,
    /// A model byte string is empty.
    #[error("local model is empty")]
    EmptyInput,
    /// No supported files were discovered.
    #[error("no .nam or .wav files were found")]
    NoSupportedFiles,
    /// The path or URL violates the local filesystem policy.
    #[error("unsafe local path: {0}")]
    UnsafePath(&'static str),
    /// NAM parsing failed.
    #[error(transparent)]
    Nam(#[from] NamError),
    /// WAV decoding failed.
    #[error(transparent)]
    Audio(#[from] AudioError),
    /// The complete A2/container gate rejected a model.
    #[error("local model validation failed: {0}")]
    Validation(#[from] DspError),
    /// Tone synthesis failed.
    #[error(transparent)]
    Tone(#[from] ToneError),
    /// Stash storage failed.
    #[error(transparent)]
    Stash(#[from] StashError),
    /// URL parsing failed.
    #[error("invalid local URL: {0}")]
    Url(#[from] url::ParseError),
    /// File or directory I/O failed.
    #[error("local I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Error returned by the content-addressed local stash.
#[derive(Debug, Error)]
pub enum StashError {
    /// File or directory I/O failed.
    #[error("stash I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// The model name or resolved path is unsafe or unsupported.
    #[error("invalid stash model name or path")]
    InvalidName,
    /// The persisted model URL is not a valid file URL.
    #[error("invalid local stash URL")]
    InvalidUrl,
    /// Input exceeds 50 MiB.
    #[error("stash input exceeds the 50 MiB limit")]
    InputTooLarge,
    /// Input is empty.
    #[error("stash input is empty")]
    EmptyInput,
}

/// Error returned by bounded byte-cache operations.
#[derive(Debug, Error)]
pub enum CacheError {
    /// One value exceeds total cache capacity.
    #[error("cache entry has {actual} bytes, exceeding the {max}-byte cache capacity")]
    EntryTooLarge {
        /// Entry size.
        actual: usize,
        /// Total cache capacity.
        max: usize,
    },
    /// A local-only operation was requested for a catalog or missing entry.
    #[error("cache operation requires an existing local entry")]
    WrongNamespace,
    /// A newer restore has already claimed publication.
    #[error("restore generation was superseded")]
    Superseded,
}

/// Error returned by a model byte fetcher.
#[derive(Debug, Error)]
pub enum FetchError {
    /// The fetch failed with a safe static diagnostic.
    #[error("model fetch failed: {0}")]
    Failed(&'static str),
    /// A successful response contained no model bytes.
    #[error("model download was empty")]
    EmptyDownload,
    /// Authentication is required or was rejected after refresh.
    #[error("model download authentication is required")]
    Unauthorized,
}

/// Error returned while coordinating model acquisition.
#[derive(Debug, Error)]
pub enum AcquireError {
    /// The byte source failed.
    #[error(transparent)]
    Fetch(#[from] FetchError),
    /// The byte cache rejected the model.
    #[error(transparent)]
    Cache(#[from] CacheError),
    /// A newer request for the same block superseded this completion.
    #[error("acquisition generation {generation} was superseded")]
    Superseded {
        /// Stale generation.
        generation: u64,
    },
    /// The generation was explicitly cancelled.
    #[error("acquisition generation {generation} was cancelled")]
    Cancelled {
        /// Cancelled generation.
        generation: u64,
    },
}

/// Bounded HTTP transport error categories.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum HttpError {
    /// Header/connect deadline expired.
    #[error("HTTP connect/header timeout")]
    ConnectTimeout,
    /// Response-body deadline expired.
    #[error("HTTP response-body timeout")]
    BodyTimeout,
    /// Transport failed without a safe detailed diagnostic.
    #[error("HTTP transport failed")]
    Transport,
    /// Response exceeded its class-specific byte bound.
    #[error("HTTP response exceeded the {max}-byte limit")]
    ResponseTooLarge {
        /// Maximum response size.
        max: usize,
    },
    /// URL scheme or origin is not allowed.
    #[error("HTTP URL scheme or origin is not allowed")]
    UnsafeOrigin,
}

/// Error returned by typed catalog clients.
#[derive(Debug, Error)]
pub enum ApiError {
    /// Transport failed.
    #[error(transparent)]
    Http(#[from] HttpError),
    /// Upstream returned a non-success status.
    #[error("catalog request failed with HTTP {0}")]
    HttpStatus(u16),
    /// Authentication is required or refresh did not produce a token.
    #[error("catalog authentication is required")]
    Unauthorized,
    /// JSON parsing failed.
    #[error("invalid catalog JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Tone conversion failed.
    #[error("invalid catalog tone: {0}")]
    Tone(ToneError),
    /// Parsed response shape is invalid.
    #[error("invalid catalog response: {0}")]
    InvalidDocument(&'static str),
    /// Response exceeded the JSON bound.
    #[error("catalog JSON exceeded the {max}-byte limit")]
    ResponseTooLarge {
        /// Maximum JSON size.
        max: usize,
    },
    /// A newer page or taxonomy request superseded this one.
    #[error("catalog generation {generation} was superseded")]
    Superseded {
        /// Stale generation.
        generation: u64,
    },
    /// The session could not give a token, but it is not signed out.
    #[error(transparent)]
    Session(#[from] SessionError),
}

/// Error returned by model and image downloads.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// Header/connect deadline expired.
    #[error("download connect/header timeout")]
    ConnectTimeout,
    /// Response-body deadline expired.
    #[error("download body timeout")]
    BodyTimeout,
    /// Transport failed without exposing sensitive diagnostics.
    #[error("download transport failed")]
    Transport,
    /// Response exceeded a byte bound.
    #[error("download exceeded the {max}-byte limit")]
    ResponseTooLarge {
        /// Maximum response size.
        max: usize,
    },
    /// Upstream returned a non-success status.
    #[error("download failed with HTTP {0}")]
    HttpStatus(u16),
    /// Authentication is required or was rejected after refresh.
    #[error("model download authentication is required")]
    Unauthorized,
    /// Successful response was empty.
    #[error("model download was empty")]
    EmptyDownload,
    /// URL scheme or origin is not allowed.
    #[error("download URL scheme or origin is not allowed")]
    UnsafeOrigin,
    /// Local stash support was not configured.
    #[error("local model stash is not configured")]
    LocalUnavailable,
    /// Image decoding or its resource limits failed.
    #[error("invalid or excessive image data")]
    Image,
    /// Stash resolution or touch failed.
    #[error(transparent)]
    Stash(#[from] StashError),
    /// Local file I/O failed.
    #[error("local download I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// The session could not give a token, but it is not signed out.
    #[error(transparent)]
    Session(#[from] SessionError),
}

/// Error returned by OAuth browser, device, and token flows.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum OAuthError {
    /// OS randomness failed.
    #[error("secure random source failed")]
    Random,
    /// Loopback listener or stream failed.
    #[error("OAuth loopback listener failed")]
    Loopback,
    /// Callback state does not match the single-use flow.
    #[error("OAuth state mismatch")]
    StateMismatch,
    /// Browser or device flow was cancelled.
    #[error("OAuth flow was cancelled")]
    Cancelled,
    /// Device code expired.
    #[error("OAuth device code expired")]
    ExpiredDeviceCode,
    /// Device authorization is still pending in the system browser.
    #[error("OAuth device authorization is pending")]
    AuthorizationPending,
    /// The server requested a slower device polling interval.
    #[error("OAuth device polling must slow down")]
    SlowDown,
    /// Session persistence failed.
    #[error("OAuth session persistence failed: {0}")]
    Session(String),
    /// Authorization-code exchange failed.
    #[error("OAuth code exchange failed")]
    ExchangeFailed,
    /// Refresh failed and the session must follow the logout rules.
    #[error("OAuth token refresh failed")]
    RefreshFailed,
    /// Device authorization failed.
    #[error("OAuth device authorization failed")]
    DeviceAuthorizationFailed,
    /// The server had a failure of its own, or it limited the rate, during a device poll or a
    /// refresh. This says nothing about the credential, and a retry can pass.
    #[error("OAuth server failed with HTTP {0}")]
    ServerFailure(u16),
    /// Token response is malformed.
    #[error("invalid OAuth token response")]
    InvalidTokenResponse,
    /// Server returned an OAuth error code.
    #[error("OAuth callback error: {0}")]
    Callback(String),
    /// Authorization code is absent.
    #[error("OAuth callback is missing a code")]
    MissingCode,
    /// Callback request is invalid or too large.
    #[error("invalid OAuth callback request")]
    InvalidCallback,
    /// Callback read deadline expired.
    #[error("OAuth callback read timed out")]
    CallbackTimeout,
    /// HTTP transport failed.
    #[error(transparent)]
    Http(#[from] HttpError),
}

/// Error returned by XML properties and settings stores.
#[derive(Debug, Error)]
pub enum PropertiesError {
    /// XML is malformed or uses an unsupported root/entry shape.
    #[error("malformed properties XML")]
    Malformed,
    /// Properties file exceeds the bounded size.
    #[error("properties file is too large")]
    TooLarge,
    /// Root-relative settings path escapes its authorized directory.
    #[error("settings path escapes its authorized directory")]
    UnsafePath,
    /// File or lock I/O failed.
    #[error("properties I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Error returned by account session persistence.
#[derive(Debug, Error)]
pub enum SessionError {
    /// OAuth refresh failed.
    #[error(transparent)]
    OAuth(#[from] OAuthError),
    /// Settings failed.
    #[error(transparent)]
    Properties(#[from] PropertiesError),
    /// No fresh or adoptable session remains; cached profile data is unchanged.
    #[error("session is offline")]
    Offline,
}

/// Error returned by bounded JUCE binary codecs.
#[derive(Debug, Error)]
pub enum CodecError {
    /// Input ended before a declared field or payload completed.
    #[error("truncated JUCE binary input")]
    Truncated,
    /// A complete tree has trailing bytes.
    #[error("trailing bytes after JUCE tree")]
    TrailingBytes,
    /// A compressed count is negative or malformed.
    #[error("invalid JUCE count")]
    InvalidCount,
    /// Text is not valid UTF-8.
    #[error("invalid UTF-8 in JUCE binary input")]
    InvalidUtf8,
    /// Variant type tag is unsupported.
    #[error("unsupported JUCE variant type tag {0}")]
    UnsupportedType(u8),
    /// A value violates the stream grammar.
    #[error("invalid JUCE value: {0}")]
    InvalidValue(&'static str),
    /// Nesting exceeds the configured limit.
    #[error("JUCE nesting exceeds depth {max}")]
    DepthLimit {
        /// Maximum depth.
        max: usize,
    },
    /// Node count exceeds the configured limit.
    #[error("JUCE node count exceeds {max}")]
    NodeLimit {
        /// Maximum nodes.
        max: usize,
    },
    /// Binary or string payload exceeds the configured limit.
    #[error("JUCE payload has {actual} bytes, exceeding {max}")]
    BlobTooLarge {
        /// Supplied payload size.
        actual: usize,
        /// Maximum payload size.
        max: usize,
    },
    /// A bounded allocation failed.
    #[error("could not allocate bounded JUCE output")]
    Allocation,
}

/// Error returned by `T3KB` session decoding or encoding.
#[derive(Debug, Error)]
pub enum StateError {
    /// Magic is not `T3KB`.
    #[error("invalid session magic")]
    InvalidMagic,
    /// Session schema is newer than this implementation.
    #[error("unsupported session schema {0}")]
    UnsupportedSchema(i64),
    /// Binary codec failed.
    #[error(transparent)]
    Codec(#[from] CodecError),
    /// Tone JSON conversion failed.
    #[error("invalid session tone: {0}")]
    Tone(ToneError),
    /// Chain or session conversion failed.
    #[error(transparent)]
    Chain(#[from] ChainError),
}

/// Error returned while planning offline restoration.
#[derive(Debug, Error)]
pub enum RestoreError {
    /// An active model has neither embedded bytes nor a retry source.
    #[error("model {model_id} in block {block_id} has no bytes or retry source")]
    MissingModel {
        /// Block identity.
        block_id: crate::BlockId,
        /// Model identity.
        model_id: crate::ModelId,
    },
    /// Byte publication failed.
    #[error(transparent)]
    Cache(#[from] CacheError),
}

/// Error returned by preset framing, naming, or atomic writes.
#[derive(Debug, Error)]
pub enum PresetError {
    /// Preset magic or framing is unsupported.
    #[error("invalid preset framing")]
    InvalidFraming,
    /// Header length, type, or fields are invalid.
    #[error("invalid preset header")]
    InvalidHeader,
    /// Preset body type is invalid.
    #[error("invalid preset body")]
    InvalidBody,
    /// Complete preset exceeds the aggregate file bound.
    #[error("preset exceeds the aggregate file-size limit")]
    InputTooLarge,
    /// User-write interface cannot replace a factory preset.
    #[error("factory presets are read-only")]
    FactoryReadOnly,
    /// Binary codec failed.
    #[error(transparent)]
    Codec(#[from] CodecError),
    /// JSON serialization failed.
    #[error("preset JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// File I/O failed.
    #[error("preset I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for NAM document operations.
pub type NamResult<T> = Result<T, NamError>;

/// Result type for model processing operations.
pub type DspResult<T> = Result<T, DspError>;
