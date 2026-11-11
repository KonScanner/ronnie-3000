#include <cstdlib>
#include <iostream>
#include <stdexcept>

#include <juce_core/juce_core.h>
#include <juce_data_structures/juce_data_structures.h>

#include "PresetFile.h"
#include "UiPrefs.h"

namespace
{
juce::File inputFile(char* path)
{
  return juce::File(juce::String::fromUTF8(path));
}

void writeOutput(const juce::File& file, const void* data, size_t size)
{
  if (!file.replaceWithData(data, size))
    throw std::runtime_error("could not write state-oracle output");
}

juce::ValueTree readTree(const juce::File& file)
{
  juce::MemoryBlock bytes;
  if (!file.loadFileAsData(bytes))
    throw std::runtime_error("could not read state-oracle input");
  const auto tree = juce::ValueTree::readFromData(bytes.getData(), bytes.getSize());
  if (!tree.isValid())
    throw std::runtime_error("invalid ValueTree input");
  return tree;
}

void writeTree(const juce::ValueTree& tree, const juce::File& file)
{
  juce::MemoryOutputStream bytes;
  tree.writeToStream(bytes);
  writeOutput(file, bytes.getData(), bytes.getDataSize());
}

void encodeTree(const juce::File& input, const juce::File& output, bool header)
{
  juce::MemoryBlock bytes;
  if (!input.loadFileAsData(bytes))
    throw std::runtime_error("could not read state-oracle input");
  const auto xml = juce::String::fromUTF8(static_cast<const char*>(bytes.getData()),
                                         static_cast<int>(bytes.getSize()));
  auto tree = juce::ValueTree::fromXml(xml);
  if (!tree.isValid() || (header && !tree.hasType(t3k::presetfile::kHeaderTag)))
    throw std::runtime_error("invalid XML ValueTree input");
  writeTree(tree, output);
}

void decodeTree(const juce::File& input, const juce::File& output, bool header)
{
  const auto tree = readTree(input);
  if (header && !tree.hasType(t3k::presetfile::kHeaderTag))
    throw std::runtime_error("unexpected header tree");
  const auto xml = tree.toXmlString();
  writeOutput(output, xml.toRawUTF8(), xml.getNumBytesAsUTF8());
}

void presetName(const juce::File& input, const juce::File& output)
{
  juce::MemoryBlock bytes;
  if (!input.loadFileAsData(bytes))
    throw std::runtime_error("could not read state-oracle input");
  const auto name = juce::String::fromUTF8(static_cast<const char*>(bytes.getData()),
                                          static_cast<int>(bytes.getSize()));
  const auto stem = t3k::presetfile::sanitizeStem(name);
  writeOutput(output, stem.toRawUTF8(), stem.getNumBytesAsUTF8());
}

void tokenMerge(const juce::File& input, const juce::File& output)
{
  juce::MemoryBlock bytes;
  if (!input.loadFileAsData(bytes))
    throw std::runtime_error("could not read state-oracle input");
  const auto text = juce::String::fromUTF8(static_cast<const char*>(bytes.getData()),
                                          static_cast<int>(bytes.getSize()));
  const auto command = juce::JSON::parse(text);
  if (!command.isObject())
    throw std::runtime_error("token-merge input must be an object");

  const auto settings = output.getSiblingFile("token-merge.settings");
  settings.deleteFile();
  juce::PropertiesFile::Options options;
  options.applicationName = "RonieStateOracle";
  options.folderName = "RonieStateOracle";
  options.filenameSuffix = "settings";
  options.osxLibrarySubFolder = "Application Support";
  juce::InterProcessLock lock("RonieStateOracle-" + juce::Uuid().toString());
  juce::PropertiesFile firstFile(settings, options);
  juce::PropertiesFile secondFile(settings, options);
  t3k::ui::UiPrefs first(&firstFile, &lock);
  t3k::ui::UiPrefs second(&secondFile, &lock);
  first.set("tokens", command["initial"].toString());
  second.set("tokens", command["external"].toString());
  first.sync();
  const auto merged = first.get("tokens");
  writeOutput(output, merged.toRawUTF8(), merged.getNumBytesAsUTF8());
}

int localLoad(const juce::File& input, const juce::File& output)
{
  const auto* configured = std::getenv("RONIE_REFERENCE_TEST_EXE");
  if (configured == nullptr || *configured == '\0')
    throw std::runtime_error("set RONIE_REFERENCE_TEST_EXE for local-load probes");
  juce::MemoryBlock bytes;
  if (!input.loadFileAsData(bytes))
    throw std::runtime_error("could not read state-oracle input");
  auto filter = juce::String::fromUTF8(static_cast<const char*>(bytes.getData()),
                                      static_cast<int>(bytes.getSize()))
                    .trim();
  if (filter.isEmpty())
    filter = "LocalLoadTest.*";
  if (!filter.startsWith("LocalLoadTest."))
    throw std::runtime_error("local-load accepts only LocalLoadTest filters");

  juce::ChildProcess process;
  if (!process.start(juce::StringArray{configured, "--gtest_filter=" + filter}))
    throw std::runtime_error("could not start local-load probe");
  const auto result = process.readAllProcessOutput();
  const auto exitCode = process.waitForProcessToFinish(120000);
  if (exitCode != 0)
    return exitCode == 0 ? 1 : exitCode;
  writeOutput(output, result.toRawUTF8(), result.getNumBytesAsUTF8());
  return 0;
}
}

int main(int argc, char** argv)
{
  try
  {
    if (argc != 4)
    {
      std::cerr << "usage: ronie-reference-state <operation> <input> <output>\n";
      return 2;
    }
    const juce::String operation = argv[1];
    const auto input = inputFile(argv[2]);
    const auto output = inputFile(argv[3]);
    if (operation == "header-encode") encodeTree(input, output, true);
    else if (operation == "header-decode") decodeTree(input, output, true);
    else if (operation == "body-encode") encodeTree(input, output, false);
    else if (operation == "body-decode") decodeTree(input, output, false);
    else if (operation == "preset-name") presetName(input, output);
    else if (operation == "token-merge") tokenMerge(input, output);
    else if (operation == "local-load") return localLoad(input, output);
    else throw std::runtime_error("unsupported state-oracle operation");
    return 0;
  }
  catch (const std::exception& error)
  {
    std::cerr << error.what() << '\n';
    return 1;
  }
}
