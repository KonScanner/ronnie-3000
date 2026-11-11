#include <algorithm>
#include <bit>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <limits>
#include <string>
#include <vector>

#include "get_dsp.h"

namespace
{
constexpr double kTau = 6.283185307179586476925286766559;

enum class Stream
{
  Silence,
  Impulse,
  Sines,
  Noise,
  Transients,
  NonFinite,
  Denormals
};

Stream parse_stream(const std::string& name)
{
  if (name == "silence") return Stream::Silence;
  if (name == "impulse") return Stream::Impulse;
  if (name == "sines") return Stream::Sines;
  if (name == "noise") return Stream::Noise;
  if (name == "transients") return Stream::Transients;
  if (name == "nonfinite") return Stream::NonFinite;
  if (name == "denormals") return Stream::Denormals;
  throw std::invalid_argument("unknown stream: " + name);
}

NAM_SAMPLE make_sample(Stream stream, size_t index, size_t frames, uint64_t& state)
{
  switch (stream)
  {
    case Stream::Silence: return 0.0;
    case Stream::Impulse: return index == 0 ? 1.0 : 0.0;
    case Stream::Sines:
    {
      const double phase = kTau * static_cast<double>(index) / 48000.0;
      return 0.5 * (std::sin(220.0 * phase) + std::sin(1230.0 * phase));
    }
    case Stream::Noise:
      state ^= state << 13;
      state ^= state >> 7;
      state ^= state << 17;
      return (static_cast<double>(state) / static_cast<double>(UINT64_MAX)) * 2.0 - 1.0;
    case Stream::Transients:
      if (index == frames / 4 || index == frames / 2) return 1.0;
      if (index == frames * 3 / 4) return -1.0;
      return 0.0;
    case Stream::NonFinite:
      if (index % 4 == 0) return std::numeric_limits<double>::quiet_NaN();
      if (index % 4 == 1) return std::numeric_limits<double>::infinity();
      if (index % 4 == 2) return -std::numeric_limits<double>::infinity();
      return 0.0;
    case Stream::Denormals:
      if (index % 3 == 0) return std::numeric_limits<double>::denorm_min();
      if (index % 3 == 1) return -std::numeric_limits<double>::denorm_min();
      return 0.0;
  }
  return 0.0;
}
}

int main(int argc, char** argv)
{
  try
  {
    if (argc != 7)
    {
      std::cerr << "usage: ronie-reference <model-file> <stream> <block-size> <frame-count> <seed> <output-raw-f64>\n";
      return 2;
    }
    static_assert(sizeof(NAM_SAMPLE) == sizeof(double));
    static_assert(std::endian::native == std::endian::little);

    const std::filesystem::path model_file = argv[1];
    const Stream stream = parse_stream(argv[2]);
    const size_t block_size = std::stoull(argv[3]);
    const size_t frame_count = std::stoull(argv[4]);
    uint64_t state = std::stoull(argv[5]);
    const std::filesystem::path output_file = argv[6];
    if (block_size == 0 || frame_count == 0)
      throw std::invalid_argument("block size and frame count must be nonzero");

    nam::DspLoadOptions options;
    options.prewarm = false;
    auto model = nam::get_dsp(model_file, options);
    model->SetPrewarmOnReset(false);
    model->Reset(48000.0, static_cast<int>(block_size));

    std::vector<NAM_SAMPLE> input(block_size);
    std::vector<NAM_SAMPLE> output(block_size);
    NAM_SAMPLE* input_pointer = input.data();
    NAM_SAMPLE* output_pointer = output.data();
    std::ofstream writer(output_file, std::ios::binary | std::ios::trunc);
    if (!writer)
      throw std::runtime_error("could not open output file");

    for (size_t start = 0; start < frame_count; start += block_size)
    {
      const size_t count = std::min(block_size, frame_count - start);
      for (size_t index = 0; index < count; ++index)
        input[index] = make_sample(stream, start + index, frame_count, state);
      model->process(&input_pointer, &output_pointer, static_cast<int>(count));
      writer.write(reinterpret_cast<const char*>(output.data()),
                   static_cast<std::streamsize>(count * sizeof(NAM_SAMPLE)));
    }
    writer.flush();
    if (!writer)
      throw std::runtime_error("could not write output file");
    return 0;
  }
  catch (const std::exception& error)
  {
    std::cerr << error.what() << '\n';
    return 1;
  }
}
