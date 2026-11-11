#include <algorithm>
#include <cstdint>
#include <cstring>
#include <exception>
#include <memory>
#include <string>
#include <vector>

#include <juce_audio_processors/juce_audio_processors.h>
#include <juce_data_structures/juce_data_structures.h>
#include "json.hpp"

#include "Processor.h"

juce::AudioProcessor* JUCE_CALLTYPE createPluginFilter();

namespace
{
using json = nlohmann::json;

juce::File inputFile(char* path)
{
  return juce::File(juce::String::fromUTF8(path));
}

juce::AudioProcessorParameter& findParameter(juce::AudioProcessor& processor,
                                             const std::string& id)
{
  for (auto* parameter : processor.getParameters())
  {
    auto* identified = dynamic_cast<juce::AudioProcessorParameterWithID*>(parameter);
    if (identified != nullptr && identified->paramID == juce::String(id))
      return *parameter;
  }
  throw std::runtime_error("unknown scenario parameter");
}

void applyParameters(juce::AudioProcessor& processor, const json& parameters)
{
  if (!parameters.is_object())
    throw std::runtime_error("scenario parameters must be an object");
  for (const auto& [id, value] : parameters.items())
  {
    const auto normalized = value.get<double>();
    if (!std::isfinite(normalized))
      throw std::runtime_error("scenario parameters must be finite");
    findParameter(processor, id).setValueNotifyingHost(static_cast<float>(normalized));
  }
}

void configureInputMode(juce::AudioProcessor& processor, const std::string& mode)
{
  if (mode == "stereo")
    return;
  if (mode != "left" && mode != "right" && mode != "dual")
    throw std::runtime_error("unknown scenario input mode");

  juce::MemoryBlock bytes;
  processor.getStateInformation(bytes);
  if (bytes.getSize() <= 4)
    throw std::runtime_error("could not capture initial processor state");
  auto state = juce::ValueTree::readFromData(static_cast<const char*>(bytes.getData()) + 4,
                                             bytes.getSize() - 4);
  if (!state.isValid())
    throw std::runtime_error("could not decode initial processor state");
  state.setProperty("inputMode", juce::String(mode), nullptr);
  juce::MemoryBlock updated;
  juce::MemoryOutputStream stream(updated, false);
  stream.write("T3KB", 4);
  state.writeToStream(stream);
  processor.setStateInformation(updated.getData(), static_cast<int>(updated.getSize()));
}

void configureOversampling(juce::AudioProcessor& processor, int factor)
{
  if (factor != 1 && factor != 2 && factor != 4 && factor != 8)
    throw std::runtime_error("invalid scenario oversampling factor");
  findParameter(processor, "osEnabled").setValueNotifyingHost(factor > 1 ? 1.0f : 0.0f);
  const auto choice = factor == 1 ? 0.0f : static_cast<float>(std::log2(factor) - 1) / 2.0f;
  findParameter(processor, "osFactor").setValueNotifyingHost(choice);
}

void prepare(juce::AudioProcessor& processor, const json& spec, int inputChannels,
             int outputChannels)
{
  const auto rate = spec.at("host_sample_rate").get<double>();
  const auto maxFrames = spec.at("max_host_frames").get<int>();
  if (!std::isfinite(rate) || rate <= 0.0 || maxFrames <= 0)
    throw std::runtime_error("invalid scenario preparation");
  processor.setPlayConfigDetails(inputChannels, outputChannels, rate, maxFrames);
  processor.prepareToPlay(rate, maxFrames);
}

void restoreIrBlock(juce::AudioProcessor& processor, const json& scenario)
{
  const auto& snapshot = scenario.at("snapshot");
  const auto& controls = snapshot.at("block");
  const auto path = snapshot.at("ir_path").get<std::string>();
  juce::MemoryBlock modelBytes;
  if (!juce::File(juce::String(path)).loadFileAsData(modelBytes))
    throw std::runtime_error("could not read IR scenario bytes");

  juce::ValueTree block("ChainBlock");
  block.setProperty("id", "blk-a", nullptr);
  block.setProperty("type", "ir", nullptr);
  block.setProperty("enabled", controls.at("enabled").get<bool>(), nullptr);
  block.setProperty("normalize", controls.at("normalize").get<bool>(), nullptr);
  block.setProperty("inputGain", controls.at("inputGain").get<double>(), nullptr);
  block.setProperty("outputGain", controls.at("outputGain").get<double>(), nullptr);
  block.setProperty("mix", controls.at("mix").get<double>(), nullptr);
  block.setProperty("toneId", 1, nullptr);
  block.setProperty("activeModelId", 100, nullptr);
  block.setProperty("toneJson", "{\"id\":1,\"title\":\"Test IR\",\"format\":\"ir\",\"gear\":\"cab\",\"models\":[{\"id\":100,\"name\":\"cab\",\"model_url\":\"https://test.invalid/cab.wav\"}]}", nullptr);
  juce::ValueTree cached("CachedModel");
  cached.setProperty("modelId", 100, nullptr);
  cached.setProperty("data", juce::var(modelBytes), nullptr);
  juce::ValueTree cache("ModelCache");
  cache.appendChild(cached, nullptr);
  block.appendChild(cache, nullptr);

  const auto& band = controls.at("eq");
  juce::ValueTree eq("Eq");
  eq.setProperty("enabled", true, nullptr);
  eq.setProperty("pre", controls.at("pre").get<bool>(), nullptr);
  juce::ValueTree bandTree("Band");
  bandTree.setProperty("type", juce::String(band.at("type").get<std::string>()), nullptr);
  bandTree.setProperty("freqHz", band.at("freqHz").get<double>(), nullptr);
  bandTree.setProperty("gainDb", band.at("gainDb").get<double>(), nullptr);
  bandTree.setProperty("q", band.at("q").get<double>(), nullptr);
  eq.appendChild(bandTree, nullptr);
  block.appendChild(eq, nullptr);
  juce::ValueTree left("ChainBlocks");
  left.appendChild(block, nullptr);
  juce::ValueTree snapshotTree("ChainSnapshot");
  snapshotTree.setProperty("stereoEnabled", false, nullptr);
  snapshotTree.appendChild(left, nullptr);
  snapshotTree.appendChild(juce::ValueTree("RightChainBlocks"), nullptr);

  juce::MemoryBlock current;
  processor.getStateInformation(current);
  if (current.getSize() <= 4)
    throw std::runtime_error("could not capture processor state");
  auto state = juce::ValueTree::readFromData(static_cast<const char*>(current.getData()) + 4,
                                             current.getSize() - 4);
  if (!state.isValid())
    throw std::runtime_error("could not decode processor state");
  state.removeChild(state.getChildWithName("ChainSnapshot"), nullptr);
  state.appendChild(snapshotTree, nullptr);
  juce::MemoryBlock updated;
  juce::MemoryOutputStream stream(updated, false);
  stream.write("T3KB", 4);
  state.writeToStream(stream);
  processor.setStateInformation(updated.getData(), static_cast<int>(updated.getSize()));
  juce::MessageManager::getInstance()->runDispatchLoopUntil(500);
}

void writeRender(const juce::File& output, const json& metadata,
                 const std::vector<float>& samples)
{
  juce::FileOutputStream stream(output);
  if (!stream.openedOk())
    throw std::runtime_error("could not open chain output");
  stream.setPosition(0);
  stream.truncate();
  stream.write("RCHAIN01", 8);
  const auto text = metadata.dump();
  const auto metadataLength = juce::ByteOrder::swapIfBigEndian(
      static_cast<std::uint64_t>(text.size()));
  stream.write(&metadataLength, sizeof(metadataLength));
  stream.write(text.data(), text.size());
  for (const float sample : samples)
  {
    std::uint32_t bits = 0;
    static_assert(sizeof(bits) == sizeof(sample));
    std::memcpy(&bits, &sample, sizeof(bits));
    bits = juce::ByteOrder::swapIfBigEndian(bits);
    stream.write(&bits, sizeof(bits));
  }
  stream.flush();
  if (stream.failedToOpen())
    throw std::runtime_error("could not flush chain output");
}

int renderEqProbe(const json& snapshot, const std::vector<size_t>& callbackFrames,
                  std::vector<float> planarInput, size_t frames, const juce::File& output)
{
  const auto& controls = snapshot.at("block");
  const auto& band = controls.at("eq");
  BlockEq eq;
  eq.prepare(48000.0);
  eq.setEnabled(true);
  eq.setPre(controls.at("pre").get<bool>());
  const auto type = band.at("type").get<std::string>();
  BlockEq::Band bandValue;
  if (type == "lowcut") bandValue.type = BlockEq::BandType::LowCut;
  else if (type == "lowshelf") bandValue.type = BlockEq::BandType::LowShelf;
  else if (type == "highshelf") bandValue.type = BlockEq::BandType::HighShelf;
  else if (type == "highcut") bandValue.type = BlockEq::BandType::HighCut;
  else bandValue.type = BlockEq::BandType::Bell;
  bandValue.freqHz = band.at("freqHz").get<float>();
  bandValue.gainDb = band.at("gainDb").get<float>();
  bandValue.q = band.at("q").get<float>();
  if (!eq.setBand(0, bandValue))
    throw std::runtime_error("could not set EQ scenario band");

  std::vector<float> planarOutput(frames * 2, 0.0f);
  size_t offset = 0;
  for (const auto callback : callbackFrames)
  {
    float* channels[2] = {planarInput.data() + offset, planarInput.data() + frames + offset};
    juce::AudioBuffer<float> buffer(channels, 2, static_cast<int>(callback));
    if (eq.isActive())
      eq.process(buffer);
    for (int channel = 0; channel < 2; ++channel)
      std::memcpy(planarOutput.data() + channel * frames + offset, channels[channel],
                  sizeof(float) * callback);
    offset += callback;
  }
  json metadata;
  metadata["channels"] = 2;
  metadata["frames"] = frames;
  metadata["latency"] = {{"samples", 0}, {"tail_seconds", 0.0}};
  metadata["parameters"] = json::object();
  metadata["diagnostic_state"] = {{"probe", "eq"}, {"active", eq.isActive()}};
  writeRender(output, metadata, planarOutput);
  return 0;
}

int renderIrProbe(const json& spec, const json& snapshot,
                  const std::vector<size_t>& callbackFrames,
                  std::vector<float> planarInput, size_t frames, size_t maxCallback,
                  const juce::File& output)
{
  const auto factor = spec.at("oversampling").get<int>();
  const bool nonuniform = snapshot.at("nonuniform").get<bool>();
  const bool stereo = snapshot.at("stereo").get<bool>();
  const auto path = snapshot.at("ir_path").get<std::string>();
  std::unique_ptr<juce::dsp::Convolution> convolver;
  if (nonuniform)
    convolver = std::make_unique<juce::dsp::Convolution>(juce::dsp::Convolution::NonUniform{8192});
  else
    convolver = std::make_unique<juce::dsp::Convolution>();
  convolver->loadImpulseResponse(juce::File(juce::String(path)),
                                 stereo ? juce::dsp::Convolution::Stereo::yes
                                        : juce::dsp::Convolution::Stereo::no,
                                 juce::dsp::Convolution::Trim::yes, 0,
                                 juce::dsp::Convolution::Normalise::no);
  const auto chunk = static_cast<int>(std::min<size_t>(maxCallback, 256));
  convolver->prepare({48000.0, static_cast<juce::uint32>(chunk), 2});
  juce::AudioBuffer<float> silence(2, chunk);
  for (int done = 0; done < 7200; done += chunk)
  {
    silence.clear();
    juce::dsp::AudioBlock<float> block(silence);
    convolver->process(juce::dsp::ProcessContextReplacing<float>(block));
  }
  const auto irSize = convolver->getCurrentIRSize();
  std::vector<float> planarOutput(frames * 2, 0.0f);
  size_t offset = 0;
  if (factor == 1)
  {
    for (const auto callback : callbackFrames)
    {
      float* channels[2] = {planarInput.data() + offset, planarInput.data() + frames + offset};
      juce::dsp::AudioBlock<float> block(channels, 2, callback);
      processConvolverInChunks(*convolver, block);
      for (int channel = 0; channel < 2; ++channel)
        std::memcpy(planarOutput.data() + channel * frames + offset, channels[channel],
                    sizeof(float) * callback);
      offset += callback;
    }
  }
  else
  {
    ChainOversampler chain;
    ChainOversampler island;
    chain.prepare(factor, static_cast<int>(maxCallback));
    island.prepare(factor, static_cast<int>(maxCallback));
    for (const auto callback : callbackFrames)
    {
      float* channels[2] = {planarInput.data() + offset, planarInput.data() + frames + offset};
      float* outputs[2] = {planarOutput.data() + offset, planarOutput.data() + frames + offset};
      chain.process(channels, outputs, static_cast<int>(callback),
                    [&](float** oversampled, float**, int) {
                      island.processBaseRateIsland(oversampled, 2,
                                                   static_cast<int>(callback * static_cast<size_t>(factor)),
                                                   [&](float* const* base, int baseFrames) {
                        juce::dsp::AudioBlock<float> block(base, 2,
                                                           static_cast<size_t>(baseFrames));
                        processConvolverInChunks(*convolver, block);
                      });
                    });
      offset += callback;
    }
  }
  json metadata;
  metadata["channels"] = 2;
  metadata["frames"] = frames;
  metadata["latency"] = {{"samples", 0}, {"tail_seconds", irSize / 48000.0}};
  metadata["parameters"] = json::object();
  metadata["diagnostic_state"] = {{"probe", "ir"}, {"irSize", irSize}};
  writeRender(output, metadata, planarOutput);
  return 0;
}

int renderRateProbe(const std::string& probe, const json& spec, const json& snapshot,
                    const std::vector<size_t>& callbackFrames,
                    std::vector<float> planarInput, size_t frames, size_t maxCallback,
                    const juce::File& output)
{
  if (probe == "eq")
    return renderEqProbe(snapshot, callbackFrames, std::move(planarInput), frames, output);
  if (probe == "ir")
    return renderIrProbe(spec, snapshot, callbackFrames, std::move(planarInput), frames,
                         maxCallback, output);
  const auto hostRate = spec.at("host_sample_rate").get<double>();
  const auto factor = spec.at("oversampling").get<int>();
  std::vector<float> planarOutput(frames * 2, 0.0f);
  size_t offset = 0;
  int latency = 0;

  if (probe == "boundary")
  {
    ChainBoundaryResampler boundary(48000.0);
    boundary.Reset(hostRate, static_cast<int>(maxCallback));
    latency = boundary.GetLatency();
    for (const auto callback : callbackFrames)
    {
      float* inputs[2] = {planarInput.data() + offset, planarInput.data() + frames + offset};
      float* outputs[2] = {planarOutput.data() + offset, planarOutput.data() + frames + offset};
      boundary.ProcessBlock(inputs, outputs, static_cast<int>(callback),
                            [](float** in, float** out, int count) {
                              for (int channel = 0; channel < 2; ++channel)
                                std::memcpy(out[channel], in[channel],
                                            sizeof(float) * static_cast<size_t>(count));
                            });
      offset += callback;
    }
  }
  else
  {
    ChainOversampler oversampler;
    oversampler.prepare(factor, static_cast<int>(maxCallback));
    for (const auto callback : callbackFrames)
    {
      float* channels[2] = {planarInput.data() + offset, planarInput.data() + frames + offset};
      float* outputs[2] = {planarOutput.data() + offset, planarOutput.data() + frames + offset};
      if (probe == "oversampling")
      {
        oversampler.process(channels, outputs, static_cast<int>(callback),
                            [](float**, float**, int) {});
      }
      else if (probe == "ir-island")
      {
        oversampler.processBaseRateIsland(channels, 2, static_cast<int>(callback),
                                          [](float* const*, int) {});
        for (int channel = 0; channel < 2; ++channel)
          std::memcpy(outputs[channel], channels[channel],
                      sizeof(float) * callback);
      }
      else
      {
        throw std::runtime_error("unknown rate probe");
      }
      offset += callback;
    }
  }

  json metadata;
  metadata["channels"] = 2;
  metadata["frames"] = frames;
  metadata["latency"] = {{"samples", latency}, {"tail_seconds", 0.0}};
  metadata["parameters"] = json::object();
  metadata["diagnostic_state"] = {{"probe", probe}, {"oversampling", factor}};
  writeRender(output, metadata, planarOutput);
  return 0;
}

int render(const juce::File& input, const juce::File& output)
{
  juce::ScopedJuceInitialiser_GUI initialise;
  const auto scenario = json::parse(input.loadFileAsString().toStdString());
  if (scenario.at("schema_version").get<unsigned>() != 1)
    throw std::runtime_error("unsupported scenario version");
  const auto& spec = scenario.at("spec");
  const auto inputChannels = spec.at("stereo_input").get<bool>() ? 2 : 1;
  const auto outputChannels = spec.at("stereo_output").get<bool>() ? 2 : 1;
  const auto callbackFrames = scenario.at("callback_frames").get<std::vector<size_t>>();
  const auto planarInput = scenario.at("input").get<std::vector<float>>();
  if (callbackFrames.empty() || planarInput.empty())
    throw std::runtime_error("scenario callbacks and input must be nonempty");
  size_t frames = 0;
  size_t maxCallback = 0;
  for (const auto callback : callbackFrames)
  {
    if (callback == 0 || callback > static_cast<size_t>(std::numeric_limits<int>::max()) ||
        frames > planarInput.size() - std::min(planarInput.size(), callback))
      throw std::runtime_error("invalid scenario callback length");
    frames += callback;
    maxCallback = std::max(maxCallback, callback);
  }
  if (frames > planarInput.size() / static_cast<size_t>(inputChannels) ||
      planarInput.size() != frames * static_cast<size_t>(inputChannels))
    throw std::runtime_error("scenario input dimensions do not match");

  const auto probe = scenario.at("snapshot").value("probe", std::string());
  if (!probe.empty() && probe != "full-ir")
  {
    if (inputChannels != 2 || outputChannels != 2)
      throw std::runtime_error("rate probes require stereo input and output");
    return renderRateProbe(probe, spec, scenario.at("snapshot"), callbackFrames, planarInput,
                           frames, maxCallback, output);
  }

  std::unique_ptr<juce::AudioProcessor> processor(createPluginFilter());
  if (processor == nullptr)
    throw std::runtime_error("could not create processor");
  configureInputMode(*processor, spec.at("input_mode").get<std::string>());
  configureOversampling(*processor, spec.at("oversampling").get<int>());
  applyParameters(*processor, scenario.at("parameters"));
  prepare(*processor, spec, inputChannels, outputChannels);
  if (probe == "full-ir")
    restoreIrBlock(*processor, scenario);

  const auto bufferChannels = std::max(inputChannels, outputChannels);
  juce::AudioBuffer<float> buffer(bufferChannels, static_cast<int>(maxCallback));
  juce::MidiBuffer midi;
  std::vector<float> planarOutput(frames * static_cast<size_t>(outputChannels), 0.0f);
  size_t offset = 0;
  for (size_t callbackIndex = 0; callbackIndex < callbackFrames.size(); ++callbackIndex)
  {
    for (const auto& change : scenario.at("state_changes"))
    {
      if (change.at("callback_index").get<size_t>() != callbackIndex)
        continue;
      applyParameters(*processor, change.at("parameters"));
      if (change.at("reprepare").get<bool>())
        prepare(*processor, spec, inputChannels, outputChannels);
    }

    const auto callback = callbackFrames[callbackIndex];
    buffer.setSize(bufferChannels, static_cast<int>(callback), false, false, true);
    buffer.clear();
    midi.clear();
    for (int channel = 0; channel < bufferChannels; ++channel)
    {
      const auto sourceChannel = std::min(channel, inputChannels - 1);
      const auto* source = planarInput.data() +
                           static_cast<size_t>(sourceChannel) * frames + offset;
      buffer.copyFrom(channel, 0, source, static_cast<int>(callback));
    }
    processor->processBlock(buffer, midi);
    for (int channel = 0; channel < outputChannels; ++channel)
    {
      const auto* rendered = buffer.getReadPointer(channel);
      std::copy(rendered, rendered + callback,
                planarOutput.begin() + static_cast<size_t>(channel) * frames + offset);
    }
    offset += callback;
  }

  json finalParameters = json::object();
  std::vector<std::string> parameterIds;
  for (const auto& [id, value] : scenario.at("parameters").items())
    parameterIds.push_back(id);
  for (const auto& change : scenario.at("state_changes"))
    for (const auto& [id, value] : change.at("parameters").items())
      parameterIds.push_back(id);
  std::sort(parameterIds.begin(), parameterIds.end());
  parameterIds.erase(std::unique(parameterIds.begin(), parameterIds.end()), parameterIds.end());
  for (const auto& id : parameterIds)
    finalParameters[id] = findParameter(*processor, id).getValue();

  json metadata;
  metadata["channels"] = outputChannels;
  metadata["frames"] = frames;
  metadata["latency"] = {{"samples", processor->getLatencySamples()},
                         {"tail_seconds", processor->getTailLengthSeconds()}};
  metadata["parameters"] = std::move(finalParameters);
  metadata["diagnostic_state"] = {
      {"stereoInput", inputChannels == 2},
      {"stereoOutput", outputChannels == 2},
      {"inputMode", spec.at("input_mode")},
      {"callbackCount", callbackFrames.size()},
      {"oversampling", spec.at("oversampling")}};
  writeRender(output, metadata, planarOutput);
  processor->releaseResources();
  return 0;
}
}

int main(int argc, char** argv)
{
  try
  {
    if (argc != 3)
      return 2;
    return render(inputFile(argv[1]), inputFile(argv[2]));
  }
  catch (const std::exception&)
  {
    return 1;
  }
}
