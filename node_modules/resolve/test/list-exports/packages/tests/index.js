'use strict';

const test = require('tape');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');
const { diffString } = require('json-diff');
const fromEntries = require('object.fromentries');
const hasDynamicImport = require('has-dynamic-import');
const getPackageType = require('get-package-type');
const getCategoriesForRange = require('node-exports-info/getCategoriesForRange');
const getRangePairs = require('node-exports-info/getRangePairs');
const arrayFrom = require('array.from');
const resolve = require('resolve/sync');

const listExports = require('list-exports');

const { OFFLINE, GREP, WRITE, SKIP_CLI } = process.env;

const fixturesDir = path.join(__dirname, 'fixtures');
const fixtures = fs.readdirSync(fixturesDir).filter((x) => !x.startsWith('.'));

const isOffline = !!OFFLINE;

const cli = path.join(__dirname, '..', 'ls-exports', 'bin', 'ls-exports');

const re = GREP && new RegExp(GREP);

function isInternalPackage(name) {
	return name === 'ls-exports' || name === 'list-exports';
}

function readExpectedJson(expectedPath, packageData) {
	try {
		const expected = JSON.parse(fs.readFileSync(expectedPath));
		if (isInternalPackage(expected.name)) {
			expected.version = packageData.version;
		}
		return expected;
	} catch (e) {
		return {};
	}
}

function diffApiOutput(t, message, { expected, results, expectedPath }) {
	const diff = diffString(expected, results);
	if (diff) {
		console.error(`# ${diffString(expected, results, { full: true }).split('\n').join('\n# ')}`);
	}
	t.deepEqual(results, expected, message);
	if (WRITE) {
		let resultsToWrite = results;
		if (isInternalPackage(expected.name)) {
			resultsToWrite = { ...resultsToWrite, version: null };
		}
		fs.writeFileSync(expectedPath, `${JSON.stringify(resultsToWrite, null, '\t').trim()}\n`);
	}
}

function serializer(key, value) {
	if (value instanceof Set) {
		return arrayFrom(value);
	}
	if (value instanceof Map) {
		return fromEntries(arrayFrom(value));
	}
	return value;
}

const currentCategories = getCategoriesForRange(process.version);
const categories = getRangePairs();

test('listExports', (t) => {
	t.plan(fixtures.length);

	fixtures.forEach((fixture) => {
		const skip = re && !re.test(fixture);
		t.test(`fixture: ${fixture}`, { skip }, async (st) => {
			const checkNPM = !isOffline && !fixture.startsWith('ex-') && !isInternalPackage(fixture);

			const fixtureDir = path.join(fixturesDir, fixture);
			const projectDir = path.join(fixtureDir, 'project');

			const packageJSON = path.resolve(path.join(projectDir, 'package.json'));
			const packageData = JSON.parse(fs.readFileSync(packageJSON));

			let found = 0;
			categories.forEach(([range, category]) => {
				const rangeMatchesCurrent = currentCategories.includes(category);
				const expectedPath = path.join(fixtureDir, 'expected', `${category}.json`);
				const exists = fs.existsSync(expectedPath);
				if (exists) {
					found += 1;
				}

				st.test(`${fixture} (${category})`, { skip: !exists && !WRITE }, async (s2t) => {
					const expected = readExpectedJson(expectedPath, packageData);

					const results = await listExports(packageJSON, { node: range }).catch((e) => {
						console.error(e);
						throw e;
					});

					s2t.test(`${fixture} (${category}): API`, (s3t) => {
						s3t.plan(1 + (rangeMatchesCurrent ? 1 : 0));

						const resultsParsed = JSON.parse(JSON.stringify(results, serializer));
						// to avoid the "engine mismatch" in "problems" in nodes that ls-exports doesn't support
						if (fixture === 'ls-exports') {
							resultsParsed.problems = [];
						}

						diffApiOutput(s3t, `${fixture} (${category}): API results match expectation`, {
							expected,
							results: resultsParsed,
							expectedPath,
						});

						if (rangeMatchesCurrent) {
							s3t.test(`${fixture} (${category}): native node resolution`, async (s4t) => {
								const resultsNative = await listExports(packageJSON).catch((e) => {
									console.error(e);
									throw e;
								});
								// to avoid the "engine mismatch" in "problems" in nodes that ls-exports doesn't support
								if (fixture === 'ls-exports') {
									resultsNative.problems = [];
								}

								const resultsNativeJSON = JSON.stringify(resultsNative, serializer);

								diffApiOutput(s4t, `${fixture} (${category}): node ${process.version}: API results match expectation`, {
									expected,
									results: JSON.parse(resultsNativeJSON),
									expectedPath,
								});

								if (!resultsNative.private) {
									const exportResults = resultsNative.exports[category];
									if (exportResults) {
										exportResults.require.forEach((file, specifier) => {
											const fixtureSpec = `@fixtures/${specifier.replace(/^\./, resultsNative.name)}`;

											const fixtureNM = fixtureSpec.split(path.sep).slice(0, 2).join(path.sep);
											const fixtureNMdir = path.dirname(resolve(path.join(fixtureNM, 'package.json'), { preserveSymlinks: true }));
											const fixtureRealDir = path.dirname(resolve(path.join(fixtureNM, 'package.json'), { preserveSymlinks: false }));

											s4t.equal(
												projectDir,
												fixtureRealDir,
												'precondition: `fixtureNMdir`’s real path is `projectDir`',
												{ skip: isInternalPackage(resultsNative.name) },
											);

											try {
												const resolvedPath = require.resolve(fixtureSpec);
												s4t.equal(
													path.relative(
														fixtureNMdir,
														resolvedPath,
													),
													path.relative(
														fixtureNMdir,
														isInternalPackage(resultsNative.name)
															? path.join(__dirname, '..', resultsNative.name, file)
															: path.join(projectDir, file),
													),
													`${fixture} (${category}): ${specifier} resolves natively`,
												);
											} catch (e) {
												if (e.code === 'ERR_INVALID_PACKAGE_TARGET') {
													s4t.comment(`# SKIP (require) ${fixtureSpec} is not a valid package target`);
												} else {
													throw e;
												}
											}
										});

										if (hasDynamicImport()) {
											await arrayFrom(exportResults.import).reduce(async (prev, [specifier, file]) => {
												await prev;

												const fixtureSpec = `@fixtures/${specifier.replace(/^\./, resultsNative.name)}`;
												const fullFilename = fs.realpathSync(path.join(__dirname, `../../node_modules/@fixtures/${file.replace(/^\./, resultsNative.name)}`));
												const ext = path.extname(fullFilename);
												const isESM = ext === '.mjs' || (ext === '.js' && getPackageType.sync(fullFilename) === 'module');
												delete require.cache[fullFilename];
												try {
													await Function('fixtureSpec', 'return import(fixtureSpec)')(fixtureSpec); // eslint-disable-line no-new-func
												} catch (e) {
													if (e.code === 'ERR_INVALID_PACKAGE_TARGET') {
														s4t.comment(`# SKIP (import) ${fixtureSpec} is not a valid package target`);
													} else {
														throw e;
													}
												}
												const mod = require.cache[fullFilename];
												s4t.equal(
													mod && mod.filename,
													fullFilename,
													`${fixture} (${category}): ${specifier} dynamic imports successfully`,
													{
														todo: !mod || mod.filename !== fullFilename,
														skip: isESM
															? 'ESM imports do not populate the CJS module cache'
															: specifier === '.'
																? 'import() fails to populate the require cache when importing CJS as a `main`'
																: path.basename(fullFilename) === 'index.js'
																	? 'import() fails to populate the require cache when importing an `index.js` CJS'
																	: false,
													},
												);
											}, Promise.resolve());
										}
									}
								}
							});
						}
					});

					if (rangeMatchesCurrent) {
						s2t.test(`${fixture} (${category}): CLI`, { skip: SKIP_CLI || !exists }, (s3t) => {
							s3t.plan(checkNPM ? 2 : 1);

							const cliResults = JSON.parse(execSync(`${cli} path "./${path.relative(process.cwd(), projectDir)}" --json`));
							s3t.deepEqual(cliResults, expected, `${fixture}: CLI results match expectation`);

							if (checkNPM) {
								const actualName = results.name.replace(/-\d+(?:\.\d+(?:\.\d+)?)?$/, '');
								const npmResults = JSON.parse(execSync(`${cli} package "${actualName}@${results.version}" --json`));
								expected.name = actualName;
								const npmDiff = diffString(expected, npmResults);
								if (npmDiff) {
									console.error(`# ${diffString(expected, npmResults, { full: true }).split('\n').join('\n# ')}`);
								}
								s3t.deepEqual(npmResults, expected, `${fixture}: npm package results match expectation`);
							}

							s3t.end();
						});
					}
				});
			});
			st.ok(found > 0, `${fixture}: at least one category is found`);
			st.equal(found, categories.length, `${fixture}: all categories have a fixture`);
		});
	});

	t.end();
});
