'use strict';

var test = require('tape');
var semver = require('semver');
var whyNotEqual = require('is-equal/why');
var forEach = require('for-each');
var resolve = require('resolve/sync');

var hasBrokenExports = semver.satisfies(process.version, '~13.0 || ~13.1', { includePrerelease: true });
var hasPackageExports = require('has-package-exports');
var hasConditions = require('has-package-exports/conditional');
// var hasPatterns = require('has-package-exports/pattern');

var getExpected = require('./conditions-expected');

var re = process.env.GREP && new RegExp(process.env.GREP);

test('condition ordering', { skip: re && !re.test('condition ordering') }, function (t) {
	if (hasBrokenExports) {
		t.ok(
			semver.satisfies(process.version, '~13.0 || ~13.1', { includePrerelease: true }),
			'node ~13.0 || ~13.1: the "exports" field should not work, but does, incorrectly, and only supports a string'
		);
	} else if (!hasPackageExports) {
		t.ok(
			semver.satisfies(process.version, '<12.17', { includePrerelease: true }),
			'node < 12.17: no support for the "exports" field'
		);
	} else if (!hasConditions) {
		t.ok(
			semver.satisfies(process.version, '13.2 - 13.6', { includePrerelease: true }),
			'node 13.2 - 13.6: supports the "exports" field‘s object form, but no conditions beyond "default"'
		);
	} else {
		t.ok(
			semver.satisfies(process.version, '^12.17 || ^13.7 || >= 14', { includePrerelease: true }),
			'node ^12.17 || ^13.7 || >= 14: supports the "exports" field'
		);
	}

	forEach([
		['require.resolve', getExpected(require.resolve)],
		['resolve', getExpected(function (x) { return resolve(x); })]
	], function (entry) {
		var desc = entry[0];
		var results = entry[1];

		t.test(desc, { todo: desc === 'resolve' || (desc === 'require.resolve' && semver.satisfies(process.version, '< 6')) }, function (st) {
			st.deepEqual(
				Object.keys(results.expected).map(function (e) { return e === '.' ? e : './' + e; }),
				Object.keys(results['package'].exports),
				'test expects proper exports'
			);

			st.equal(whyNotEqual(results.expected, results.actual), '', 'expected exported values match actual exported values');
			st.deepEqual(results.expected, results.actual);

			st.end();
		});
	});

	t.end();
});
