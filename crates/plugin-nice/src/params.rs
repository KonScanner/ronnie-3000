//! Host parameters, built from the Ronnie parameter catalog.
//!
//! The host gets one parameter for each catalog entry that a session or a preset keeps. A machine
//! setting, such as the input calibration, stays out of the host. A continuous parameter holds the
//! knob position from 0 to 1, so host automation follows the same curve as the Ronnie knob. A
//! switch is a host switch, and a choice is a stepped host parameter that holds the index of its
//! label. So a host shows them as switches and steps, not as sliders.
//!
//! The host IDs are `p_1`, `p_2`, and so on, in catalog order. A saved project keeps these IDs, so
//! the catalog must only grow at its end. A test pins the list.

use std::sync::{
    Arc, Mutex, MutexGuard, PoisonError, Weak,
    atomic::{AtomicU32, Ordering},
};

use base64::Engine as _;
use nice_plug::{context::gui::GuiContext, params::persist::PersistentField, prelude::*};
use ronie_core::params::{ParameterCatalog, ParameterSpec, ParameterValue};
use ronie_ui::UiBackend;

/// The smallest host value change that counts. The session rounds some values to a step, and a
/// rounding must not look like an edit.
const HOST_EPSILON: f32 = 5e-4;

/// One host parameter, of the type that fits its catalog entry.
pub(crate) enum HostParam {
    /// A knob position from 0 to 1.
    Knob(FloatParam),
    /// A whole number: the index of the label of a choice, or the value of another stepped entry.
    Choice(IntParam),
    /// A switch.
    Switch(BoolParam),
}

impl HostParam {
    /// The host value: a knob position, the index of a choice, or 0 or 1 for a switch.
    ///
    /// This allocates nothing, so the audio thread can call it.
    #[must_use]
    pub(crate) fn value(&self) -> f32 {
        match self {
            Self::Knob(param) => param.value(),
            Self::Choice(param) => param.value() as f32,
            Self::Switch(param) => f32::from(u8::from(param.value())),
        }
    }

    /// Gives the host the value `host` as one gesture, as if the user changed it in the editor.
    fn set_in_gesture(&self, setter: &ParamSetter<'_>, host: f32) {
        match self {
            Self::Knob(param) => {
                setter.begin_set_parameter(param);
                setter.set_parameter(param, host);
                setter.end_set_parameter(param);
            }
            Self::Choice(param) => {
                setter.begin_set_parameter(param);
                setter.set_parameter(param, host.round() as i32);
                setter.end_set_parameter(param);
            }
            Self::Switch(param) => {
                setter.begin_set_parameter(param);
                setter.set_parameter(param, host >= 0.5);
                setter.end_set_parameter(param);
            }
        }
    }
}

// SAFETY: The one pointer points to the parameter inside `self`. nice-plug reads it only while the
// parameters live, which is the same contract as for a derived `Params`. The ID `p` becomes `p_N`
// in the array of `RonnieParams`, as the earlier derived struct gave. The derive macro cannot make a
// parameter whose type depends on the catalog entry, so this impl is written out.
#[allow(unsafe_code)]
unsafe impl Params for HostParam {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let pointer = match self {
            Self::Knob(param) => param.as_ptr(),
            Self::Choice(param) => param.as_ptr(),
            Self::Switch(param) => param.as_ptr(),
        };
        vec![("p".to_owned(), pointer, String::new())]
    }
}

/// How one host parameter maps to its catalog entry.
pub(crate) struct Binding {
    /// The catalog entry.
    pub(crate) spec: &'static ParameterSpec,
    /// Whether the host value is the session value itself, for a switch or a choice.
    discrete: bool,
    /// The bits of the host value that the worker saw last. Only the worker uses it.
    host_seen: AtomicU32,
    /// The bits of the session value that the editor saw last. Only the editor uses it.
    session_seen: AtomicU32,
}

impl Binding {
    /// The session value of a host value.
    #[must_use]
    pub(crate) fn to_session(&self, host: f32) -> ParameterValue {
        ParameterValue(if self.discrete {
            host
        } else {
            self.spec.value_at(host)
        })
    }

    /// The host value of a session value.
    #[must_use]
    pub(crate) fn to_host(&self, session: ParameterValue) -> f32 {
        if self.discrete {
            session.0
        } else {
            self.spec.position_of(session.0)
        }
    }

    /// Records `host` as the host value that the worker saw. Returns whether it changed.
    fn host_changed(&self, host: f32) -> bool {
        self.host_seen.swap(host.to_bits(), Ordering::AcqRel) != host.to_bits()
    }

    /// Forgets the host value that the worker saw, so that the next pass gives it again.
    fn forget_host(&self) {
        // No host value has the bits of this NaN, so the next value is a change.
        self.host_seen.store(f32::NAN.to_bits(), Ordering::Release);
    }

    /// Records `session` as the session value that the editor saw. Returns whether it changed.
    fn session_changed(&self, session: ParameterValue) -> bool {
        self.session_seen
            .swap(session.0.to_bits(), Ordering::AcqRel)
            != session.0.to_bits()
    }
}

/// All plugin parameters.
#[derive(Params)]
pub struct RonnieParams {
    /// The host parameters, in catalog order.
    #[nested(array)]
    pub(crate) values: Vec<HostParam>,
    /// The catalog entry of each host parameter, in the same order.
    pub(crate) bindings: Vec<Binding>,
    /// The session, which the host saves in its project.
    #[persist = "session"]
    pub session: SessionField,
    /// The editor zoom, which the host saves in its project. The editor opens at this zoom.
    #[persist = "editor-zoom"]
    editor_zoom: Mutex<f32>,
}

/// The session as a field of the host project: the session bytes, with the model bytes that its
/// blocks use, as base64 text.
pub struct SessionField {
    backend: Arc<ronie_plugin::ProcessorBackend>,
    /// The bytes of the last save, with their text. An unchanged session gives the same bytes.
    ///
    /// The processor keeps the bytes, so this field keeps only a weak link to them. A weak link
    /// also keeps the memory of the bytes reserved, so no new bytes can get the same address.
    /// The text is about 4/3 of the size of the session, with its models.
    text: Mutex<Option<(Weak<[u8]>, String)>>,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The guarded value stays valid after a panic in another holder.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PersistentField<'_, String> for SessionField {
    /// Restores the session and compiles no model. The engine compiles the chain for the stream.
    fn set(&self, encoded: String) {
        let restored = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                self.backend
                    .restore_host_session_bytes(&bytes)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = restored {
            nice_error!("Ronnie could not restore the session of the project: {error}");
        }
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&String) -> R,
    {
        self.map_bytes(self.backend.session_bytes(), f)
    }
}

impl SessionField {
    /// Gives `f` the text of `bytes`. When the encode failed, `f` gets the text of the last save,
    /// so that the host does not replace the last good session with an empty one.
    fn map_bytes<F, R>(&self, bytes: Result<Arc<[u8]>, ronie_ui::backend::UiError>, f: F) -> R
    where
        F: Fn(&String) -> R,
    {
        let mut text = lock(&self.text);
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                nice_error!("Ronnie could not save the session in the project: {error}");
                let empty = String::new();
                return f(text.as_ref().map_or(&empty, |(_, encoded)| encoded));
            }
        };
        match &*text {
            Some((known, encoded)) if std::ptr::addr_eq(known.as_ptr(), Arc::as_ptr(&bytes)) => {
                f(encoded)
            }
            _ => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let (_, encoded) = text.insert((Arc::downgrade(&bytes), encoded));
                f(encoded)
            }
        }
    }
}

/// Tells the host about each session value that changed, for example by an edit in the view.
///
/// Call this on the editor thread after each pass. Each side of the sync acts only on a change on
/// its own side: this function on a changed session value, and [`take_host_changes`] on a changed
/// host value. A session value that the host already has makes no gesture, so a change that came
/// from the host does not go back to it.
pub(crate) fn publish_session_changes(
    backend: &dyn UiBackend,
    params: &RonnieParams,
    gui: &GuiContext,
) {
    let setter = gui.param_setter();
    for (param, binding) in params.values.iter().zip(&params.bindings) {
        let Some(session) = backend.parameter(binding.spec.id) else {
            continue;
        };
        if !binding.session_changed(session) {
            continue;
        }
        let host = binding.to_host(session);
        if (host - param.value()).abs() > HOST_EPSILON {
            param.set_in_gesture(&setter, host);
        }
    }
}

/// Gives the session each host value that changed, with no undo step.
///
/// A value that the session refuses is given again at the next pass.
///
/// Call this on a control thread, never on the audio thread, because it locks the session.
pub(crate) fn take_host_changes(backend: &ronie_plugin::ProcessorBackend, params: &RonnieParams) {
    for (param, binding) in params.values.iter().zip(&params.bindings) {
        let host = param.value();
        if binding.host_changed(host)
            && backend
                .automate_parameter(binding.spec.id, binding.to_session(host))
                .is_err()
        {
            binding.forget_host();
        }
    }
}

/// Whether the host gets a parameter for `spec`.
#[must_use]
pub(crate) fn is_host_parameter(spec: &ParameterSpec) -> bool {
    spec.scope.preset || !spec.scope.machine_default
}

impl RonnieParams {
    /// The parameters of the session that `backend` holds.
    #[must_use]
    pub fn new(backend: Arc<ronie_plugin::ProcessorBackend>) -> Self {
        let specs = ParameterCatalog::v1()
            .iter()
            .filter(|spec| is_host_parameter(spec));
        let (values, bindings) = specs.map(host_parameter).unzip();
        Self {
            values,
            bindings,
            session: SessionField {
                backend,
                text: Mutex::new(None),
            },
            editor_zoom: Mutex::new(1.0),
        }
    }

    /// The editor zoom that the project holds, in the zoom limits. A value that is not finite
    /// gives 1.
    #[must_use]
    pub(crate) fn editor_zoom(&self) -> f32 {
        ronie_ui::shell_view::ShellView::clamp_zoom(*lock(&self.editor_zoom))
    }

    /// Records the editor zoom for the next save of the project.
    #[cfg(test)]
    pub(crate) fn set_editor_zoom(&self, zoom: f32) {
        *lock(&self.editor_zoom) = zoom;
    }

    /// Records `zoom` for the next save of the project, if the project still holds `known`.
    ///
    /// A project restore can write the zoom between the read and this write. That zoom stays, and
    /// the editor asks for it at its next pass. Returns whether the write occurred.
    pub(crate) fn replace_editor_zoom(&self, known: f32, zoom: f32) -> bool {
        let mut saved = lock(&self.editor_zoom);
        let current = ronie_ui::shell_view::ShellView::clamp_zoom(*saved);
        if current.to_bits() != known.to_bits() {
            return false;
        }
        *saved = ronie_ui::shell_view::ShellView::clamp_zoom(zoom);
        true
    }
}

/// The host parameter and the binding of one catalog entry.
fn host_parameter(spec: &'static ParameterSpec) -> (HostParam, Binding) {
    let discrete = !spec.choices.is_empty() || spec.interval == Some(1.0);
    let mut binding = Binding {
        spec,
        discrete,
        host_seen: AtomicU32::new(0),
        session_seen: AtomicU32::new(spec.default.to_bits()),
    };
    let default = binding.to_host(ParameterValue(spec.default));
    binding.host_seen = AtomicU32::new(default.to_bits());
    let name = display_name(spec.id.0);
    let switch = spec.choices.is_empty() && spec.min == 0.0 && spec.max == 1.0;
    let param = if discrete && !switch {
        // A choice holds the index of its label. Another whole-number entry holds its value.
        let range = IntRange::Linear {
            min: spec.min.round() as i32,
            max: spec.max.round() as i32,
        };
        let param = IntParam::new(name, default.round() as i32, range)
            .with_value_to_string(Arc::new(move |index| {
                display_value(spec, true, index as f32)
            }))
            .with_string_to_value(Arc::new(move |text| {
                parse_value(spec, true, text).map(|index| index as i32)
            }));
        HostParam::Choice(if spec.automatable {
            param
        } else {
            param.non_automatable()
        })
    } else if discrete {
        let param = BoolParam::new(name, default >= 0.5)
            .with_value_to_string(Arc::new(move |on| {
                display_value(spec, true, f32::from(u8::from(on)))
            }))
            .with_string_to_value(Arc::new(move |text| {
                parse_value(spec, true, text).map(|value| value >= 0.5)
            }));
        HostParam::Switch(if spec.automatable {
            param
        } else {
            param.non_automatable()
        })
    } else {
        let param = FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
            .with_value_to_string(Arc::new(move |host| display_value(spec, false, host)))
            .with_string_to_value(Arc::new(move |text| parse_value(spec, false, text)));
        HostParam::Knob(if spec.automatable {
            param
        } else {
            param.non_automatable()
        })
    };
    (param, binding)
}

/// A readable name from a catalog ID: `toneBass` becomes `Tone Bass`.
fn display_name(id: &str) -> String {
    let mut name = String::with_capacity(id.len() + 4);
    for (index, character) in id.chars().enumerate() {
        if index == 0 {
            name.extend(character.to_uppercase());
        } else {
            if character.is_uppercase() {
                name.push(' ');
            }
            name.push(character);
        }
    }
    name
}

/// The host value of a text that a user types in the host, in the units that the host shows.
fn parse_value(spec: &ParameterSpec, discrete: bool, text: &str) -> Option<f32> {
    let text = text.trim();
    if let Some(index) = spec
        .choices
        .iter()
        .position(|choice| choice.eq_ignore_ascii_case(text))
    {
        return Some(index as f32);
    }
    if discrete && spec.min == 0.0 && spec.max == 1.0 {
        match text.to_ascii_lowercase().as_str() {
            "on" => return Some(1.0),
            "off" => return Some(0.0),
            _ => {}
        }
    }
    let value: f32 = text.parse().ok()?;
    let value = value.clamp(spec.min, spec.max);
    Some(if discrete {
        value.round()
    } else {
        spec.position_of(value)
    })
}

/// The text that the host shows for a host value.
fn display_value(spec: &ParameterSpec, discrete: bool, host: f32) -> String {
    if !spec.choices.is_empty() {
        let index = host.round().max(0.0) as usize;
        return spec.choices.get(index).copied().unwrap_or("").to_owned();
    }
    if discrete && spec.min == 0.0 && spec.max == 1.0 {
        return if host >= 0.5 { "On" } else { "Off" }.to_owned();
    }
    let value = if discrete { host } else { spec.value_at(host) };
    format!("{value:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_ids_keep_their_catalog_entries() {
        let params = test_params();
        let ids: Vec<_> = params
            .bindings
            .iter()
            .map(|binding| binding.spec.id.0)
            .collect();
        // A saved project binds to the position of each entry. Add new entries only at the end.
        assert_eq!(
            ids, PINNED_IDS,
            "the host parameter order changed, which breaks saved projects"
        );
        let map = params.param_map();
        assert_eq!(map.len(), PINNED_IDS.len());
        assert_eq!(map[0].0, "p_1");
    }

    #[test]
    fn a_host_shows_switches_and_choices_as_steps() {
        let params = test_params();
        let map = params.param_map();
        let mut kinds = [0_usize; 3];
        for ((param, binding), (id, pointer, _)) in
            params.values.iter().zip(&params.bindings).zip(&map)
        {
            let spec = binding.spec;
            let (kind, fits) = match (param, pointer) {
                (HostParam::Choice(_), ParamPtr::IntParam(_)) => {
                    // A range of one value divides by zero when the host normalizes it.
                    let steps = spec.choices.len() != 1 && spec.max > spec.min;
                    (0, spec.interval == Some(1.0) && steps)
                }
                (HostParam::Switch(_), ParamPtr::BoolParam(_)) => (
                    1,
                    spec.choices.is_empty()
                        && spec.interval == Some(1.0)
                        && spec.min == 0.0
                        && spec.max == 1.0,
                ),
                (HostParam::Knob(_), ParamPtr::FloatParam(_)) => (2, spec.interval != Some(1.0)),
                _ => (3, false),
            };
            assert!(fits, "{id} ({}) has the wrong host type", spec.id.0);
            kinds[kind] += 1;
            let default = binding.to_host(ParameterValue(spec.default));
            assert!(
                (param.value() - default).abs() < 1e-6,
                "{}: {} is not the default {default}",
                spec.id.0,
                param.value()
            );
        }
        assert!(kinds.iter().all(|count| *count > 0), "{kinds:?}");
    }

    #[test]
    fn a_machine_setting_is_not_a_host_parameter() {
        let params = test_params();
        let ids: Vec<_> = params
            .bindings
            .iter()
            .map(|binding| binding.spec.id.0)
            .collect();
        for machine in ["inputCalibrationLevel", "osEnabled", "osFactor"] {
            assert!(!ids.contains(&machine), "{machine}");
        }
    }

    #[test]
    fn a_host_value_maps_to_the_session_value_and_back() {
        let params = test_params();
        for binding in &params.bindings {
            let spec = binding.spec;
            let host = binding.to_host(ParameterValue(spec.default));
            let back = binding.to_session(host).0;
            assert!((back - spec.default).abs() < 1e-3, "{}: {back}", spec.id.0);
        }
    }

    #[test]
    fn a_typed_value_is_read_in_the_units_that_the_host_shows() {
        let params = test_params();
        let bass = params
            .bindings
            .iter()
            .find(|binding| binding.spec.id.0 == "toneBass")
            .expect("bass");
        let host = parse_value(bass.spec, bass.discrete, " 8 ").expect("number");
        assert!((bass.to_session(host).0 - 8.0).abs() < 1e-3);
        let gate = params
            .bindings
            .iter()
            .find(|binding| binding.spec.id.0 == "gateEnabled")
            .expect("gate");
        assert_eq!(parse_value(gate.spec, gate.discrete, "Off"), Some(0.0));
        let window = params
            .bindings
            .iter()
            .find(|binding| binding.spec.id.0 == "pitchWindow")
            .expect("window");
        let label = window.spec.choices[1];
        assert_eq!(parse_value(window.spec, window.discrete, label), Some(1.0));
        assert_eq!(parse_value(bass.spec, bass.discrete, "loud"), None);
    }

    #[test]
    fn each_side_of_the_sync_acts_only_on_its_own_change() {
        let params = test_params();
        let binding = &params.bindings[0];
        let default = binding.to_host(ParameterValue(binding.spec.default));
        assert!(
            !binding.host_changed(default),
            "the default is no host change"
        );
        assert!(binding.host_changed(0.9));
        assert!(
            !binding.host_changed(0.9),
            "the same value again is no change"
        );
        assert!(!binding.session_changed(ParameterValue(binding.spec.default)));
        assert!(binding.session_changed(ParameterValue(0.25)));
    }

    #[test]
    fn a_refused_host_value_is_given_again() {
        let params = test_params();
        let binding = &params.bindings[0];
        assert!(binding.host_changed(0.9));

        binding.forget_host();

        assert!(binding.host_changed(0.9), "the next pass gives it again");
        assert!(!binding.host_changed(0.9));
    }

    #[test]
    fn a_failed_save_keeps_the_last_saved_session() {
        use ronie_ui::backend::UiError;

        let params = test_params();
        let failed = || Err(UiError::Control("test".to_owned()));
        assert_eq!(
            params.session.map_bytes(failed(), Clone::clone),
            "",
            "with no earlier save, there is no text"
        );
        let saved = params.session.map(Clone::clone);

        let kept = params.session.map_bytes(failed(), Clone::clone);

        assert!(!saved.is_empty());
        assert_eq!(kept, saved);
    }

    #[test]
    fn a_camel_case_id_reads_as_words() {
        assert_eq!(display_name("toneBass"), "Tone Bass");
        assert_eq!(display_name("inputLevel"), "Input Level");
    }

    #[test]
    fn a_saved_session_comes_back_in_a_new_plugin() {
        use ronie_core::params::ParameterId;
        use ronie_ui::backend::GesturePhase;

        let saved = test_params();
        saved
            .session
            .backend
            .set_parameter(
                ParameterId("toneBass"),
                ParameterValue(8.0),
                GesturePhase::End,
            )
            .expect("parameter");
        let encoded = saved.session.map(Clone::clone);
        // An empty session has no model bytes, so it stays small in the project.
        assert!(
            !encoded.is_empty() && encoded.len() < 64 * 1024,
            "{}",
            encoded.len()
        );

        let restored = test_params();
        restored.session.set(encoded);

        let bass = restored.session.backend.parameter(ParameterId("toneBass"));
        assert_eq!(bass, Some(ParameterValue(8.0)));
    }

    #[test]
    fn a_project_restore_leaves_the_compile_to_the_engine() {
        use ronie_core::params::ParameterId;
        use ronie_plugin::SessionRestore;

        let saved = test_params();
        saved
            .session
            .backend
            .automate_parameter(ParameterId("toneBass"), ParameterValue(8.0))
            .expect("parameter");
        let restored = test_params();

        restored.session.set(saved.session.map(Clone::clone));

        let restore = restored.session.backend.take_restore().expect("lock");
        assert!(
            matches!(restore, Some(SessionRestore::Compile)),
            "{restore:?}"
        );
    }

    #[test]
    fn an_unchanged_session_is_encoded_only_once() {
        use ronie_core::params::ParameterId;

        let params = test_params();
        let backend = &params.session.backend;
        let first = backend.session_bytes().expect("bytes");
        let again = backend.session_bytes().expect("bytes");
        assert!(
            Arc::ptr_eq(&first, &again),
            "the same session, the same bytes"
        );
        let text = params.session.map(Clone::clone);
        assert_eq!(params.session.map(Clone::clone), text);

        backend
            .automate_parameter(ParameterId("toneBass"), ParameterValue(8.0))
            .expect("parameter");

        let changed = backend.session_bytes().expect("bytes");
        assert!(!Arc::ptr_eq(&first, &changed), "a change encodes again");
        assert_ne!(params.session.map(Clone::clone), text);

        // A restore brings a new saved document, also with the same bytes.
        backend
            .restore_host_session_bytes(&changed)
            .expect("restore");
        let restored = backend.session_bytes().expect("bytes");
        assert!(!Arc::ptr_eq(&changed, &restored), "a restore encodes again");
    }

    #[test]
    fn the_editor_zoom_comes_back_from_the_project() {
        let saved = test_params();
        saved.set_editor_zoom(1.25);
        let fields = saved.serialize_fields();
        assert!(fields.contains_key("editor-zoom"), "{:?}", fields.keys());

        let restored = test_params();
        restored.deserialize_fields(&fields);

        assert!((restored.editor_zoom() - 1.25).abs() < f32::EPSILON);
    }

    #[test]
    fn a_damaged_session_leaves_the_session_as_it_is() {
        use ronie_core::params::ParameterId;

        let params = test_params();
        params.session.set("not base64 !".to_owned());
        params.session.set(String::new());

        let bass = params.session.backend.parameter(ParameterId("toneBass"));
        assert_eq!(bass, Some(ParameterValue(5.0)));
    }

    fn test_params() -> RonnieParams {
        RonnieParams::new(Arc::new(ronie_plugin::ProcessorBackend::new(
            ronie_plugin::RonieProcessor::new(ronie_dsp::PrepareSpec {
                host_sample_rate: 48_000.0,
                max_host_frames: 256,
                oversampling: 1,
                input_mode: ronie_core::InputMode::Left,
                stereo_input: false,
                stereo_output: true,
            }),
        )))
    }

    /// The host parameter order of the first release.
    const PINNED_IDS: &[&str] = &[
        "inputLevel",
        "toneBass",
        "toneMid",
        "toneTreble",
        "outputLevel",
        "gateThreshold",
        "targetLoudness",
        "gateEnabled",
        "toneEqEnabled",
        "outputBalance",
        "spreadEnabled",
        "spreadOffset",
        "spreadWobble",
        "spreadWobbleEnabled",
        "spreadCrossover",
        "spreadCrossoverEnabled",
        "spreadDiffuseEnabled",
        "alignEnabled",
        "alignOffset",
        "alignWobble",
        "alignWobbleEnabled",
        "alignCrossover",
        "alignCrossoverEnabled",
        "alignDiffuseEnabled",
        "chainPanLeft",
        "chainPanRight",
        "chainPanLinked",
        "chainSoloLeft",
        "chainSoloRight",
        "chainInvertLeft",
        "chainInvertRight",
        "gateRelease",
        "gateHold",
        "gateRange",
        "pitchEnabled",
        "pitchSemitones",
        "pitchStep",
        "pitchTonality",
        "pitchWindow",
    ];
}
