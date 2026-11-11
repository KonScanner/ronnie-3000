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
source_dir="$RONIE_REFERENCE_BUILD/ronie-reference-state-source"
build_dir="$RONIE_REFERENCE_BUILD/ronie-reference-state-build"
mkdir -p "$source_dir" "$build_dir"

cat > "$source_dir/CMakeLists.txt" <<EOF
cmake_minimum_required(VERSION 3.22)
project(ronie_reference_state_oracle LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
find_path(JUCE_SOURCE_DIR
  NAMES modules/juce_core/juce_core.h
  HINTS
    "$RONIE_REFERENCE_ROOT/libs/juce"
    "$RONIE_REFERENCE_BUILD/_deps/juce-src"
    "$RONIE_REFERENCE_BUILD/juce-src"
  NO_DEFAULT_PATH
  REQUIRED
)
add_subdirectory("\${JUCE_SOURCE_DIR}" juce)
add_executable(ronie-reference-state
  "$repo_root/crates/oracle/cpp/state_main.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/src/PresetFile.cpp"
  "$RONIE_REFERENCE_ROOT/plugin/ui/services/UiPrefs.cpp"
)
target_include_directories(ronie-reference-state PRIVATE
  "$RONIE_REFERENCE_ROOT/plugin/include"
  "$RONIE_REFERENCE_ROOT/plugin/ui/services"
)
target_compile_definitions(ronie-reference-state PRIVATE
  JUCE_STANDALONE_APPLICATION=1
  JUCE_USE_CURL=0
  JUCE_WEB_BROWSER=0
)
target_link_libraries(ronie-reference-state PRIVATE juce::juce_data_structures)
set_target_properties(ronie-reference-state PROPERTIES
  RUNTIME_OUTPUT_DIRECTORY "$RONIE_REFERENCE_BUILD"
)
EOF

cmake -S "$source_dir" -B "$build_dir" -DCMAKE_BUILD_TYPE=Release
cmake --build "$build_dir" --target ronie-reference-state --parallel
