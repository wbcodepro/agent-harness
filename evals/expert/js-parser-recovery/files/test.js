'use strict';
// Tests of the parser as it is today. Run with: node test.js
const assert = require('assert');
const { parse, show } = require('./parser.js');

assert.strictEqual(show(parse('1 + 2 * 3')), '(+ 1 (* 2 3))');
assert.strictEqual(show(parse('(1 + 2) * 3')), '(* (+ 1 2) 3)');
assert.strictEqual(show(parse('-a - -b')), '(- (neg a) (neg b))');
assert.strictEqual(show(parse('max(1, x / 2)')), '(call max 1 (/ x 2))');
assert.strictEqual(show(parse('f()')), '(call f)');
assert.throws(() => parse('1 +'), SyntaxError);
assert.throws(() => parse('1 $ 2'), SyntaxError);
console.log('ok');
