#!/usr/bin/env bash
set -euo pipefail

: "${RONIE_REFERENCE_ROOT:?set the private baseline source directory}"
: "${RONIE_REFERENCE_BUILD:?set the private baseline build directory}"

expected_commit=ab5e5ae4388753184687d67816815aa481f2cea9
actual_commit=$(git -C "$RONIE_REFERENCE_ROOT" rev-parse HEAD)
if [[ "$actual_commit" != "$expected_commit" ]]; then
  echo "baseline source must use approved commit $expected_commit" >&2
  exit 1
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_dir="$RONIE_REFERENCE_BUILD/ronie-reference-chain-source"
build_dir="$RONIE_REFERENCE_BUILD/ronie-reference-chain-build"
mkdir -p "$source_dir" "$build_dir"

cat > "$source_dir/CMakeLists.txt" <<EOF
cmake_minimum_required(VERSION 3.22)
project(ronie_reference_chain_oracle VERSION 0.1.0 LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
add_compile_options(-fsigned-char)
add_subdirectory("$RONIE_REFERENCE_ROOT/libs/juce" juce)
set(NAM_SRC "$RONIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/NAM")
add_library(NAM STATIC
  \${NAM_SRC}/activations.cpp
  \${NAM_SRC}/conv1d.cpp
  \${NAM_SRC}/container.cpp
  \${NAM_SRC}/convnet.cpp
  \${NAM_SRC}/dsp.cpp
  \${NAM_SRC}/get_dsp.cpp
  \${NAM_SRC}/lstm.cpp
  \${NAM_SRC}/ring_buffer.cpp
  \${NAM_SRC}/util.cpp
  \${NAM_SRC}/wavenet/a2_fast.cpp
  \${NAM_SRC}/wavenet/model.cpp
  \${NAM_SRC}/wavenet/slimmable.cpp
)
target_include_directories(NAM PUBLIC
  "$RONIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore"
  "$RONIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/Dependencies/eigen"
  "$RONIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/Dependencies/nlohmann"
)
target_compile_options(NAM PRIVATE -fPIC)
target_compile_definitions(NAM PRIVATE NAM_ENABLE_A2_FAST)
set_target_properties(NAM PROPERTIES
  C_VISIBILITY_PRESET hidden
  CXX_VISIBILITY_PRESET hidden
  VISIBILITY_INLINES_HIDDEN TRUE
)
if(CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
  target_compile_options(NAM PRIVATE -fno-gnu-unique)
endif()
juce_add_console_app(ronie-reference-chain PRODUCT_NAME "RoniebaselineChain")
target_sources(ronie-reference-chain PRIVATE
  "$repo_root/crates/oracle/cpp/chain_main.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/AutoOffset.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/BlockEq.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/BlockSpectrum.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/LegacyParamIds.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/MidiMapper.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/NamEngine.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/PresetFile.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/PresetManager.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/Processor.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/ProcessorChain.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/ProcessorHistory.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/ProcessorModelLoader.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/ProcessorPresets.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/ProcessorState.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/Spread.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/StandaloneStateAutosave.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/PitchShift.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/StereoOffset.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/TunerDetector.cpp"
)
target_include_directories(ronie-reference-chain PRIVATE
  "$RONIE_REFERENCE_ROOT/plugin/include"
  "$RONIE_REFERENCE_ROOT/plugin/AudioDSPTools"
  "$RONIE_REFERENCE_ROOT/plugin/AudioDSPTools/dsp/ResamplingContainer/Dependencies"
  "$RONIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore"
  "$RONIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/Dependencies/eigen"
  "$RONIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/Dependencies/nlohmann"
)
target_compile_definitions(ronie-reference-chain PRIVATE
  JUCE_STANDALONE_APPLICATION=1
  JUCE_WEB_BROWSER=0
  JUCE_USE_WEBP=0
  JUCE_USE_CURL=0
  JUCE_MODAL_LOOPS_PERMITTED=1
  HEADLESS=1
  NAM_ENABLE_A2_FAST
  T3K_TEST_FILES_DIR="$RONIE_REFERENCE_ROOT/test/files"
)
target_link_libraries(ronie-reference-chain PRIVATE
  juce::juce_dsp
  juce::juce_audio_formats
  juce::juce_audio_processors
  juce::juce_recommended_config_flags
  -Wl,--whole-archive
  NAM
  -Wl,--no-whole-archive
)
add_dependencies(ronie-reference-chain NAM)
set_target_properties(ronie-reference-chain PROPERTIES
  RUNTIME_OUTPUT_DIRECTORY "$RONIE_REFERENCE_BUILD"
)
EOF

cmake -S "$source_dir" -B "$build_dir" -DCMAKE_BUILD_TYPE=Release
cmake --build "$build_dir" --target ronie-reference-chain --parallel

fixture_hashes="$RONIE_REFERENCE_BUILD/official-chain-fixture-hashes.txt"
: > "$fixture_hashes"
for name in \
  a2-amp-test.nam \
  a2-amp-cab-test.nam \
  a2-am-test-2.nam \
  cab-ir-test.wav \
  cab-ir-test-2.wav \
  reverb-ir-mono-test.wav \
  reverb-ir-stereo-test.wav; do
  sha256sum "$RONIE_REFERENCE_ROOT/test/files/$name" >> "$fixture_hashes"
done
