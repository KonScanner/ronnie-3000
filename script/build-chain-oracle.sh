#!/usr/bin/env bash
set -euo pipefail

: "${RONNIE_REFERENCE_ROOT:?set the private baseline source directory}"
: "${RONNIE_REFERENCE_BUILD:?set the private baseline build directory}"

expected_commit=ab5e5ae4388753184687d67816815aa481f2cea9
actual_commit=$(git -C "$RONNIE_REFERENCE_ROOT" rev-parse HEAD)
if [[ "$actual_commit" != "$expected_commit" ]]; then
  echo "baseline source must use approved commit $expected_commit" >&2
  exit 1
fi

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_dir="$RONNIE_REFERENCE_BUILD/ronnie-reference-chain-source"
build_dir="$RONNIE_REFERENCE_BUILD/ronnie-reference-chain-build"
mkdir -p "$source_dir" "$build_dir"

cat > "$source_dir/CMakeLists.txt" <<EOF
cmake_minimum_required(VERSION 3.22)
project(ronnie_reference_chain_oracle VERSION 0.1.0 LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
add_compile_options(-fsigned-char)
add_subdirectory("$RONNIE_REFERENCE_ROOT/libs/juce" juce)
set(NAM_SRC "$RONNIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/NAM")
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
  "$RONNIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore"
  "$RONNIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/Dependencies/eigen"
  "$RONNIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/Dependencies/nlohmann"
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
juce_add_console_app(ronnie-reference-chain PRODUCT_NAME "RonniebaselineChain")
target_sources(ronnie-reference-chain PRIVATE
  "$repo_root/crates/oracle/cpp/chain_main.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/AutoOffset.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/BlockEq.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/BlockSpectrum.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/LegacyParamIds.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/MidiMapper.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/NamEngine.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/PresetFile.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/PresetManager.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/Processor.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/ProcessorChain.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/ProcessorHistory.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/ProcessorModelLoader.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/ProcessorPresets.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/ProcessorState.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/Spread.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/StandaloneStateAutosave.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/PitchShift.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/StereoOffset.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/TunerDetector.cpp"
)
target_include_directories(ronnie-reference-chain PRIVATE
  "$RONNIE_REFERENCE_ROOT/plugin/include"
  "$RONNIE_REFERENCE_ROOT/plugin/AudioDSPTools"
  "$RONNIE_REFERENCE_ROOT/plugin/AudioDSPTools/dsp/ResamplingContainer/Dependencies"
  "$RONNIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore"
  "$RONNIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/Dependencies/eigen"
  "$RONNIE_REFERENCE_ROOT/plugin/NeuralAmpModelerCore/Dependencies/nlohmann"
)
target_compile_definitions(ronnie-reference-chain PRIVATE
  JUCE_STANDALONE_APPLICATION=1
  JUCE_WEB_BROWSER=0
  JUCE_USE_WEBP=0
  JUCE_USE_CURL=0
  JUCE_MODAL_LOOPS_PERMITTED=1
  HEADLESS=1
  NAM_ENABLE_A2_FAST
  T3K_TEST_FILES_DIR="$RONNIE_REFERENCE_ROOT/test/files"
)
target_link_libraries(ronnie-reference-chain PRIVATE
  juce::juce_dsp
  juce::juce_audio_formats
  juce::juce_audio_processors
  juce::juce_recommended_config_flags
  -Wl,--whole-archive
  NAM
  -Wl,--no-whole-archive
)
add_dependencies(ronnie-reference-chain NAM)
set_target_properties(ronnie-reference-chain PROPERTIES
  RUNTIME_OUTPUT_DIRECTORY "$RONNIE_REFERENCE_BUILD"
)
EOF

cmake -S "$source_dir" -B "$build_dir" -DCMAKE_BUILD_TYPE=Release
cmake --build "$build_dir" --target ronnie-reference-chain --parallel

fixture_hashes="$RONNIE_REFERENCE_BUILD/official-chain-fixture-hashes.txt"
: > "$fixture_hashes"
for name in \
  a2-amp-test.nam \
  a2-amp-cab-test.nam \
  a2-am-test-2.nam \
  cab-ir-test.wav \
  cab-ir-test-2.wav \
  reverb-ir-mono-test.wav \
  reverb-ir-stereo-test.wav; do
  sha256sum "$RONNIE_REFERENCE_ROOT/test/files/$name" >> "$fixture_hashes"
done
