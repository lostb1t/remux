# ls-exports <sup>[![Version Badge][npm-version-svg]][package-url]</sup>

[![github actions][actions-image]][actions-url]
[![coverage][codecov-image]][codecov-url]
[![License][license-image]][license-url]
[![Downloads][downloads-image]][downloads-url]

[![npm badge][npm-badge-png]][package-url]

Given a package name and a version number, or a path to a package.json, what specifiers does it expose?

The package export defaults an `async function`. It fulfills with an object with the following keys:
  - `name` the package name
  - `version`: the package version
  - `engines`: the package's `engines` requirements
  - `binaries`: the files that are made available as executable programs
  - `errors`: any validation errors encountered during parsing. <sub>Note that these errors *do not* necessarily interfere with the listed entry points being accessible at runtime.</sub>

For ESM-supporting node versions (at the time of this writing, `^12.17 || >= 13.2`):
  - `require`: valid specifiers to pass into `require`
  - `import`: valid specifiers to pass into `import()`, or to use in a static `import` statement
  - `files`: all files on the filesystem that are directly exposed by the above entry points
  - `tree`: a hierarchical object structure where each directory is represented as a key containing an object, and each file is represented as a key containing a list of the entry points that expose that file

For node versions prior to ESM support (at the time of this writing, `< 12.17 || ~13.0 || ~13.1`):
  - `require (pre-exports)`: valid specifiers to pass into `require`
  - `files (pre-exports)`: all files on the filesystem that are directly exposed by the above entry points
  - `tree (pre-exports)`: a hierarchical object structure where each directory is represented as a key containing an object, and each file is represented as a key containing a list of the entry points that expose that file

## Example

```sh
ls-exports package resolve@1
```

[package-url]: https://npmjs.org/package/ls-exports
[npm-version-svg]: https://versionbadg.es/ljharb/list-exports.svg
[deps-svg]: https://david-dm.org/ljharb/list-exports.svg
[deps-url]: https://david-dm.org/ljharb/list-exports
[dev-deps-svg]: https://david-dm.org/ljharb/list-exports/dev-status.svg
[dev-deps-url]: https://david-dm.org/ljharb/list-exports#info=devDependencies
[npm-badge-png]: https://nodei.co/npm/ls-exports.png?downloads=true&stars=true
[license-image]: https://img.shields.io/npm/l/ls-exports.svg
[license-url]: LICENSE
[downloads-image]: https://img.shields.io/npm/dm/ls-exports.svg
[downloads-url]: https://npm-stat.com/charts.html?package=ls-exports
[codecov-image]: https://codecov.io/gh/ljharb/list-exports/branch/main/graphs/badge.svg
[codecov-url]: https://app.codecov.io/gh/ljharb/list-exports/
[actions-image]: https://img.shields.io/endpoint?url=https://github-actions-badge-u3jn4tfpocch.runkit.sh/ljharb/list-exports
[actions-url]: https://github.com/ljharb/list-exports/actions
[category]: https://github.com/inspect-js/node-exports-info#categories
