'use strict';

const fs = require('fs');
const path = require('path');
const assert = require('assert');
const { diffString } = require('json-diff');

const isTest = process.env.NODE_ENV === 'test';

const conditions = ['default', 'node', 'require', 'import'];
const conditionsMap = Object.fromEntries(conditions.map((c) => [c.charAt(0), c]));

// from https://staff.roguecc.edu/JMiller/JavaScript/permute.html
const permutations = [];
const usedChars = [];
// eslint-disable-next-line func-style
function permute(input) {
	// convert input into a char array (one element for each character)
	const chars = input.split('');
	for (let i = 0; i < chars.length; i++) {
		// get and remove character at index "i" from char array
		const ch = chars.splice(i, 1);
		// add removed character to the end of used characters
		usedChars.push(ch);
		// when there are no more characters left in char array to add, add used chars to list of permutations
		if (chars.length === 0) {
			permutations[permutations.length] = usedChars.join('');
		}
		// send characters (minus the removed one from above) from char array to be permuted
		permute(chars.join(''));
		// add removed character back into char array in original position
		chars.splice(i, 0, ch);
		// remove the last character used off the end of used characters array
		usedChars.pop();
	}
}
permute(Object.keys(conditionsMap).join(''));

const pkg = {
	bundleDependencies: true,
	name: 'ex-conditions',
	version: '0.0.0',
	main: './main.js',
	exports: {
		'.': [
			{
				'default': './default.js'
			},
			'./fallback.js'
		],
		'./package.json': './package.json',
		...Object.fromEntries(permutations.map((word) => {
			const chars = word.split('');
			return [`./${word}`, [
				Object.fromEntries(chars.map((c) => [conditionsMap[c], `./${conditionsMap[c]}.${conditionsMap[c] === 'import' ? 'm' : ''}js`])),
				'./fallback.js'
			]];
		}))
	},
	permutations: permutations
};

const pkgJSONpath = path.join(__dirname, 'package.json');
const pkgJSONcontents = JSON.stringify(pkg, null, '\t').replace(/\n?$/g, '\n');
if (isTest) {
	var actual = String(fs.readFileSync(pkgJSONpath));
	assert.equal(actual, pkgJSONcontents, pkgJSONpath);
} else {
	fs.writeFileSync(pkgJSONpath, pkgJSONcontents);
}

permutations.forEach((permutation) => {
	const permPath = path.join(__dirname, `${permutation}.js`);
	const contents = `'use strict';

module.exports = '${permutation}';
`;
	if (isTest) {
		const actualContents = String(fs.readFileSync(permPath));
		assert.equal(actualContents, contents);
	} else {
		fs.writeFileSync(permPath, contents);
	}
});
