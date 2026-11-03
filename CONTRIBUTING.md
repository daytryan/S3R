# Contributing

Pull requests are welcome: fixes, gameplay parity, tools, docs and mods. For a bigger change, open an issue or ask on
the [Discord](https://discord.gg/PC2W9K4xan) first so nobody does the same work twice.

## Building

See [Build from source](README.md#build-from-source-developers) in the README. You need your own game files to build
the game's assets and play it.

## Tests

Run the tests one crate at a time, for example `cargo test -p skate-core`, rather than the whole workspace at once:
a full run needs a lot of memory. The pipeline's Python tests run one file at a time from the repository root, for
example `py -3 -m unittest tools.pipeline.test_sources`. Tests that need game files skip themselves when the files
aren't there.

## Never commit game content

No ISOs, DLC packages, `default.xex`, extracted or converted retail files (models, textures, audio, maps, scripts,
tables made from the discs) or screenshots of retail art. `.gitignore` already keeps the usual folders out; don't
force-add ignored files. Keep personal folder paths out too: scripts read their folders from environment variables.

## License

The project is under the [MIT License](LICENSE). By sending a pull request you agree that your contribution is
released under it too.
