#!/usr/bin/env bash
set -euo pipefail

: "${RONIE_REFERENCE_ROOT:?set the private baseline source directory}"
: "${RONIE_REFERENCE_BUILD:?set the private baseline build directory}"

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_dir="$RONIE_REFERENCE_BUILD/ronie-reference-source"
mkdir -p "$source_dir"

cat > "$source_dir/CMakeLists.txt" <<EOF
cmake_minimum_required(VERSION 3.20)
project(ronie_reference_oracle LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if("${RONIE_REFERENCE_A2_FAST:-ON}" STREQUAL "ON")
  add_compile_definitions(NAM_ENABLE_A2_FAST)
endif()
file(GLOB NAM_SOURCES_TOP "$RONIE_REFERENCE_ROOT/NAM/*.cpp" "$RONIE_REFERENCE_ROOT/NAM/*.c")
file(GLOB NAM_SOURCES_SUB "$RONIE_REFERENCE_ROOT/NAM/*/*.cpp")
add_executable(ronie-reference "$repo_root/crates/oracle/cpp/main.cpp" \${NAM_SOURCES_TOP} \${NAM_SOURCES_SUB})
target_include_directories(ronie-reference PRIVATE
  "$RONIE_REFERENCE_ROOT"
  "$RONIE_REFERENCE_ROOT/NAM"
  "$RONIE_REFERENCE_ROOT/Dependencies/eigen"
  "$RONIE_REFERENCE_ROOT/Dependencies/nlohmann"
)
target_link_libraries(ronie-reference PRIVATE stdc++fs)
EOF

cmake -S "$source_dir" -B "$RONIE_REFERENCE_BUILD" -DCMAKE_BUILD_TYPE=Release
cmake --build "$RONIE_REFERENCE_BUILD" --target ronie-reference --parallel
