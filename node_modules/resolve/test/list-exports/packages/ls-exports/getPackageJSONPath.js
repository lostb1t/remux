'use strict';

const { promisify } = require('util');
const path = require('path');

const npa = require('npm-package-arg');
const pacote = require('pacote');
const { dir } = require('tmp');

const tmpDir = promisify(dir);

module.exports = async function getPackageJSONPath(specifier) {
	const { name } = npa(specifier);

	const cwd = await tmpDir();

	const packageDir = path.join(cwd, 'node_modules', name);
	await pacote.extract(specifier, packageDir);

	return path.join(packageDir, 'package.json');
};
