# Changelog

## [0.6.0](https://github.com/reconbot/status-comment/compare/v0.5.0...v0.6.0) (2026-10-13)


### ⚠ BREAKING CHANGES

* the version input is removed.

### Features

* install the CLI release matching the action's version ([#9](https://github.com/reconbot/status-comment/issues/9)) ([cd2b327](https://github.com/reconbot/status-comment/commit/cd2b327aa13eb46a9c88fa4b086bd51259830767))

## [0.5.0](https://github.com/reconbot/status-comment/compare/v0.4.0...v0.5.0) (2026-10-13)


### ⚠ BREAKING CHANGES

* `status.<status>=<label>` is now `status.<status>.display=<label>`, and the config's `status` entries are objects (`{"display": …, "show-output": …}`). Old config files no longer load.
* so many edge case behaviors are now improved, biggest is the install version behavior
* move display names to config ([#5](https://github.com/reconbot/status-comment/issues/5))

### Features

* configure if a steps output is displayed by default ([#8](https://github.com/reconbot/status-comment/issues/8)) ([1603267](https://github.com/reconbot/status-comment/commit/16032671250d9454e1cc152a73e7ff849c381867))
* move display names to config ([#5](https://github.com/reconbot/status-comment/issues/5)) ([e94d53a](https://github.com/reconbot/status-comment/commit/e94d53aebe62445edf5cd3009bcdf2f855c4290c))
* show command output, store github tokens, and more! ([#7](https://github.com/reconbot/status-comment/issues/7)) ([6615605](https://github.com/reconbot/status-comment/commit/661560545fa1f53bd1fced33b1f40d010251113d))

## [0.4.0](https://github.com/reconbot/status-comment/compare/v0.3.0...v0.4.0) (2026-10-12)


### ⚠ BREAKING CHANGES

* auto finalize and enhancements ([#3](https://github.com/reconbot/status-comment/issues/3))

### Features

* auto finalize and enhancements ([#3](https://github.com/reconbot/status-comment/issues/3)) ([6eba32e](https://github.com/reconbot/status-comment/commit/6eba32e3d8b7891277daf028b9315fa79a7c303e))

## [0.3.0](https://github.com/reconbot/status-comment/compare/v0.2.0...v0.3.0) (2026-10-12)


### Features

* release workflows, install action and Apache-2.0 license ([519e39f](https://github.com/reconbot/status-comment/commit/519e39fc49d174c43eb15038891c7cd7185b5faf))


### Bug Fixes

* published version sohuldn't be prefixed ([18e72cf](https://github.com/reconbot/status-comment/commit/18e72cf35131160eecd45ec54ba53abaaab54728))

## [0.2.0](https://github.com/reconbot/status-comment/compare/status-comment-v0.1.0...status-comment-v0.2.0) (2026-10-12)


### Features

* release workflows, install action and Apache-2.0 license ([519e39f](https://github.com/reconbot/status-comment/commit/519e39fc49d174c43eb15038891c7cd7185b5faf))
