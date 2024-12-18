'use strict';

const {
	table: makeTable,
	getBorderCharacters,
} = require('table');

module.exports = function table(data, options = {}) {
	return makeTable(data, {
		border: getBorderCharacters('norc'),
		...options,
	});
};
