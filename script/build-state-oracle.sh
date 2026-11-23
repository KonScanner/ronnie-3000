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
source_dir="$RONNIE_REFERENCE_BUILD/ronnie-reference-state-source"
build_dir="$RONNIE_REFERENCE_BUILD/ronnie-reference-state-build"
mkdir -p "$source_dir" "$build_dir"

cat > "$source_dir/CMakeLists.txt" <<EOF
cmake_minimum_required(VERSION 3.22)
project(ronnie_reference_state_oracle LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
find_path(JUCE_SOURCE_DIR
  NAMES modules/juce_core/juce_core.h
  HINTS
    "$RONNIE_REFERENCE_ROOT/libs/juce"
    "$RONNIE_REFERENCE_BUILD/_deps/juce-src"
    "$RONNIE_REFERENCE_BUILD/juce-src"
  NO_DEFAULT_PATH
  REQUIRED
)
add_subdirectory("\${JUCE_SOURCE_DIR}" juce)
add_executable(ronnie-reference-state
  "$repo_root/crates/oracle/cpp/state_main.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/src/PresetFile.cpp"
  "$RONNIE_REFERENCE_ROOT/plugin/ui/services/UiPrefs.cpp"
)
target_include_directories(ronnie-reference-state PRIVATE
  "$RONNIE_REFERENCE_ROOT/plugin/include"
  "$RONNIE_REFERENCE_ROOT/plugin/ui/services"
)
target_compile_definitions(ronnie-reference-state PRIVATE
  JUCE_STANDALONE_APPLICATION=1
  JUCE_USE_CURL=0
  JUCE_WEB_BROWSER=0
)
target_link_libraries(ronnie-reference-state PRIVATE juce::juce_data_structures)
set_target_properties(ronnie-reference-state PROPERTIES
  RUNTIME_OUTPUT_DIRECTORY "$RONNIE_REFERENCE_BUILD"
)
EOF

cmake -S "$source_dir" -B "$build_dir" -DCMAKE_BUILD_TYPE=Release
cmake --build "$build_dir" --target ronnie-reference-state --parallel
