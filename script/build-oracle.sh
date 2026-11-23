#!/usr/bin/env bash
set -euo pipefail

: "${RONNIE_REFERENCE_ROOT:?set the private baseline source directory}"
: "${RONNIE_REFERENCE_BUILD:?set the private baseline build directory}"

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_dir="$RONNIE_REFERENCE_BUILD/ronnie-reference-source"
mkdir -p "$source_dir"

cat > "$source_dir/CMakeLists.txt" <<EOF
cmake_minimum_required(VERSION 3.20)
project(ronnie_reference_oracle LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if("${RONNIE_REFERENCE_A2_FAST:-ON}" STREQUAL "ON")
  add_compile_definitions(NAM_ENABLE_A2_FAST)
endif()
file(GLOB NAM_SOURCES_TOP "$RONNIE_REFERENCE_ROOT/NAM/*.cpp" "$RONNIE_REFERENCE_ROOT/NAM/*.c")
file(GLOB NAM_SOURCES_SUB "$RONNIE_REFERENCE_ROOT/NAM/*/*.cpp")
add_executable(ronnie-reference "$repo_root/crates/oracle/cpp/main.cpp" \${NAM_SOURCES_TOP} \${NAM_SOURCES_SUB})
target_include_directories(ronnie-reference PRIVATE
  "$RONNIE_REFERENCE_ROOT"
  "$RONNIE_REFERENCE_ROOT/NAM"
  "$RONNIE_REFERENCE_ROOT/Dependencies/eigen"
  "$RONNIE_REFERENCE_ROOT/Dependencies/nlohmann"
)
target_link_libraries(ronnie-reference PRIVATE stdc++fs)
EOF

cmake -S "$source_dir" -B "$RONNIE_REFERENCE_BUILD" -DCMAKE_BUILD_TYPE=Release
cmake --build "$RONNIE_REFERENCE_BUILD" --target ronnie-reference --parallel
