'use strict';

var fs = require('fs');
var path = require('path');
var semver = require('semver');
var entries = require('object.entries');
var fromEntries = require('object.fromentries');

var hasBrokenExports = semver.satisfies(process.version, '~13.0 || ~13.1', { includePrerelease: true });
var hasPackageExports = require('has-package-exports');
var hasConditions = require('has-package-exports/conditional');

var conditionsPkg = JSON.parse(String(fs.readFileSync(path.join(__dirname, './fixtures/ex-conditions/project/package.json'))));

var empty = {};

function makeResult(slug) {
	return {
		resolved: path.basename(path.join(__dirname, 'fixtures/ex-conditions/project/' + slug + '.js')),
		result: slug
	};
}

module.exports = function getExpectedConditions(resolve) {
	var expected = {
		'.': makeResult(hasBrokenExports ? 'fallback' : hasPackageExports ? 'default' : 'main'),
		'package.json': {
			resolved: path.basename(resolve('./fixtures/ex-conditions/project/package.json')),
			result: empty
		},
		dnri: makeResult(hasPackageExports ? hasBrokenExports ? 'fallback' : 'default' : 'dnri'),
		dnir: makeResult(hasPackageExports ? hasBrokenExports ? 'fallback' : 'default' : 'dnir'),
		drni: makeResult(hasPackageExports ? hasBrokenExports ? 'fallback' : 'default' : 'drni'),
		drin: makeResult(hasPackageExports ? hasBrokenExports ? 'fallback' : 'default' : 'drin'),
		dinr: makeResult(hasPackageExports ? hasBrokenExports ? 'fallback' : 'default' : 'dinr'),
		dirn: makeResult(hasPackageExports ? hasBrokenExports ? 'fallback' : 'default' : 'dirn'),
		ndri: makeResult(hasPackageExports ? hasConditions ? 'node' : hasBrokenExports ? 'fallback' : 'default' : 'ndri'),
		ndir: makeResult(hasPackageExports ? hasConditions ? 'node' : hasBrokenExports ? 'fallback' : 'default' : 'ndir'),
		nrdi: makeResult(hasPackageExports ? hasConditions ? 'node' : hasBrokenExports ? 'fallback' : 'default' : 'nrdi'),
		nrid: makeResult(hasPackageExports ? hasConditions ? 'node' : hasBrokenExports ? 'fallback' : 'default' : 'nrid'),
		nidr: makeResult(hasPackageExports ? hasConditions ? 'node' : hasBrokenExports ? 'fallback' : 'default' : 'nidr'),
		nird: makeResult(hasPackageExports ? hasConditions ? 'node' : hasBrokenExports ? 'fallback' : 'default' : 'nird'),
		rdni: makeResult(hasPackageExports ? hasConditions ? 'require' : hasBrokenExports ? 'fallback' : 'default' : 'rdni'),
		rdin: makeResult(hasPackageExports ? hasConditions ? 'require' : hasBrokenExports ? 'fallback' : 'default' : 'rdin'),
		rndi: makeResult(hasPackageExports ? hasConditions ? 'require' : hasBrokenExports ? 'fallback' : 'default' : 'rndi'),
		rnid: makeResult(hasPackageExports ? hasConditions ? 'require' : hasBrokenExports ? 'fallback' : 'default' : 'rnid'),
		ridn: makeResult(hasPackageExports ? hasConditions ? 'require' : hasBrokenExports ? 'fallback' : 'default' : 'ridn'),
		rind: makeResult(hasPackageExports ? hasConditions ? 'require' : hasBrokenExports ? 'fallback' : 'default' : 'rind'),
		idnr: makeResult(hasPackageExports ? hasBrokenExports ? 'fallback' : 'default' : 'idnr'),
		idrn: makeResult(hasPackageExports ? hasBrokenExports ? 'fallback' : 'default' : 'idrn'),
		indr: makeResult(hasPackageExports ? hasConditions ? 'node' : hasBrokenExports ? 'fallback' : 'default' : 'indr'),
		inrd: makeResult(hasPackageExports ? hasConditions ? 'node' : hasBrokenExports ? 'fallback' : 'default' : 'inrd'),
		irdn: makeResult(hasPackageExports ? hasConditions ? 'require' : hasBrokenExports ? 'fallback' : 'default' : 'irdn'),
		irnd: makeResult(hasPackageExports ? hasConditions ? 'require' : hasBrokenExports ? 'fallback' : 'default' : 'irnd')
	};

	var actual = fromEntries(entries(expected).map(function (entry) {
		var result;
		var resolved;
		try {
			var exportPath = '@fixtures/ex-conditions' + (entry[0] === '.' ? '' : '/' + entry[0]);
			// eslint-disable-next-line global-require
			result = entry[0] === 'package.json' ? empty : require(exportPath);
			resolved = path.basename(resolve(exportPath));
		} catch (e) {
			result = e;
		}
		return [entry[0], {
			resolved: resolved,
			result: result
		}];
	}));

	return {
		actual: actual,
		'package': conditionsPkg,
		expected: expected
	};
};
