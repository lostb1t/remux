# list-exports <sup>[![Version Badge][npm-version-svg]][package-url]</sup>

[![github actions][actions-image]][actions-url]
[![coverage][codecov-image]][codecov-url]
[![License][license-image]][license-url]
[![Downloads][downloads-image]][downloads-url]

[![npm badge][npm-badge-png]][package-url]

Given a path to a package.json, what specifiers does it expose?

The package export defaults an `async function`. It takes a path to a `package.json` as the only required argument.

It fulfills with an object with the following structure:
  - `name` the package name
  - `version`: the package version
  - `engines`: the package's `engines` requirements
  - `problems`: a Set of strings describing problems or validation issues encountered during exports traversal. <sub>Note that these errors *do not* necessarily interfere with the listed entry points being accessible at runtime.</sub>
  - `exports`: an object with the following structure:
    - `binaries`: a Map of executable program names, to the relative file path that name will execute.
	- `latest`: a string describing the latest “[category][]” in the given node version range. This [category][] will be present in the following list.
	- `...categories`: each [category][] that the node version range overlaps will have an object with this structure:
	  - `import`: a Map of import specifier, to relative file path
	  - `require`: a Map of require specifier, to relative file path
	  - `files`: a Set of relative file paths that are included in `import` and/or `require`
	  - `tree`: a Map. Its keys are filenames (no leading `./`), whose values are a Set of `import` or `require` specifiers that point to it; or, directory names, whose values are a Map of the same recursive structure as `tree` itself.
	- `pre-exports`: this [category][] will always be present, whether it's in the above list or not, with the above structure.

In addition to the required `package.json` path, it also takes a second argument, an options object. This object supports the following properties:
 - `node`: either `true`, which reads the `engines.node` field in `package.json`, or a valid semver range of node versions to target. Defaults to the current node version.

## Example

<details>
<summary>`const expected`:</summary>

```js
const expected = {
	name: 'list-exports',
	version: '1.1.0',
	engines: { node: '^18.17.0 || >=20.5.0' },
	problems: new Set(),
	exports: {
		binaries: {},
		latest: 'pattern-trailers-no-dir-slash',
		'pattern-trailers-no-dir-slash': {
			import: new Map([
				['.', './index.js'],
			]),
			require: new Map([
				['.', './index.js'],
				['./package.json', './package.json'],
			]),
			files: new Set([
				'./index.js',
				'./package.json',
			]),
			tree: new Map([
				['index.js', new Set(['.'])],
				['package.json', new Set(['./package.json'])],
			]),
		},
		'pre-exports': {
			import: new Map(),
			require: new Map([
				['.', './index.js'],
				['./', './index.js'],
				['./index', './index.js'],
				['./index.js', './index.js'],
				['./package', './package.json'],
				['./package.json', './package.json'],
			]),
			files: new Set([
				'./index.js',
				'./package.json',
			]),
			tree: new Map([
				['index.js', new Set([
					'.',
					'./',
					'./index.js',
					'./index',
				])],
				['package.json', new Set([
					'./package.json',
					'./package',
				])],
			]),
		},
	},
};
```
</details>

```js
const assert = require('assert');
const path = require('path');
const listExports = require('list-exports');

listExports('./package.json', { node: true }).then((data) => {
	assert.deepEqual(data, expected);
}).catch((e) => {
	console.error(e);
	process.exit(1);
});
```

[package-url]: https://npmjs.org/package/list-exports
[npm-version-svg]: https://versionbadg.es/ljharb/list-exports.svg
[deps-svg]: https://david-dm.org/ljharb/list-exports.svg
[deps-url]: https://david-dm.org/ljharb/list-exports
[dev-deps-svg]: https://david-dm.org/ljharb/list-exports/dev-status.svg
[dev-deps-url]: https://david-dm.org/ljharb/list-exports#info=devDependencies
[npm-badge-png]: https://nodei.co/npm/list-exports.png?downloads=true&stars=true
[license-image]: https://img.shields.io/npm/l/list-exports.svg
[license-url]: LICENSE
[downloads-image]: https://img.shields.io/npm/dm/list-exports.svg
[downloads-url]: https://npm-stat.com/charts.html?package=list-exports
[codecov-image]: https://codecov.io/gh/ljharb/list-exports/branch/main/graphs/badge.svg
[codecov-url]: https://app.codecov.io/gh/ljharb/list-exports/
[actions-image]: https://img.shields.io/endpoint?url=https://github-actions-badge-u3jn4tfpocch.runkit.sh/ljharb/list-exports
[actions-url]: https://github.com/ljharb/list-exports/actions
[category]: https://github.com/inspect-js/node-exports-info#categories
