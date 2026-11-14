'use strict';
// Checks parser.js against SPEC.md. Run with: node test.js
const { parse } = require('./parser.js');

function show(node) {
  if (node === null || typeof node !== 'object') throw new Error('not a node: ' + JSON.stringify(node));
  switch (node.type) {
    case 'num':
      if (typeof node.value !== 'number') throw new Error('num value is not a number');
      return String(node.value);
    case 'var': return node.name;
    case 'neg': return `(neg ${show(node.arg)})`;
    case 'bin': return `(${node.op} ${show(node.left)} ${show(node.right)})`;
    case 'call': return `(call ${[node.name, ...node.args.map(show)].join(' ')})`;
    case 'error': return '<error>';
    default: throw new Error(`unknown node type ${node.type}`);
  }
}

// [source, tree, errors as [code, pos]]
const CASES = [
  ["1 + 2 * 3","(+ 1 (* 2 3))",[]],
  ["(1 + 2) * 3","(* (+ 1 2) 3)",[]],
  ["-a - -b","(- (neg a) (neg b))",[]],
  ["max(1, x / 2)","(call max 1 (/ x 2))",[]],
  ["f()","(call f)",[]],
  ["3.25 * x1","(* 3.25 x1)",[]],
  ["2 ** 3","(** 2 3)",[]],
  ["2 ** 3 ** 2","(** 2 (** 3 2))",[]],
  ["-2 ** 2","(neg (** 2 2))",[]],
  ["2 ** -1","(** 2 (neg 1))",[]],
  ["2 ** -3 ** 2","(** 2 (neg (** 3 2)))",[]],
  ["a * b ** c / d","(/ (* a (** b c)) d)",[]],
  ["(a ** b) ** c","(** (** a b) c)",[]],
  ["--a ** b","(neg (neg (** a b)))",[]],
  ["f(x) ** g(y) ** 2","(** (call f x) (** (call g y) 2))",[]],
  ["a ?? b","(?? a b)",[]],
  ["a ?? b ?? c","(?? (?? a b) c)",[]],
  ["a ?? b + 1","(?? a (+ b 1))",[]],
  ["a + b ?? c * d ?? -e","(?? (?? (+ a b) (* c d)) (neg e))",[]],
  ["f(a ?? b, c) ?? d","(?? (call f (?? a b) c) d)",[]],
  ["a ?? (b ?? c)","(?? a (?? b c))",[]],
  ["2**3??4","(?? (** 2 3) 4)",[]],
  ["a ** b ?? c ** d","(?? (** a b) (** c d))",[]],
  ["","<error>",[["E_OPERAND",0]]],
  ["   ","<error>",[["E_OPERAND",3]]],
  ["1 +","(+ 1 <error>)",[["E_OPERAND",3]]],
  ["1 + * 2","(+ 1 (* <error> 2))",[["E_OPERAND",4]]],
  ["(1 + 2","(+ 1 2)",[["E_CLOSE",6]]],
  ["((1 + 2) * 3","(* (+ 1 2) 3)",[["E_CLOSE",12]]],
  ["f(1 2, 3) * 4","(* (call f 1) 4)",[["E_CLOSE",4]]],
  ["1 2","1",[["E_EXTRA",2]]],
  ["(1 + ) * 2 # 3","(* (+ 1 <error>) 2)",[["E_OPERAND",5],["E_CHAR",11],["E_EXTRA",13]]],
  ["1 + )","(+ 1 <error>)",[["E_OPERAND",4]]],
  [")","<error>",[["E_OPERAND",0]]],
  ["f(1,)","(call f 1 <error>)",[["E_OPERAND",4]]],
  ["f(,1)","(call f <error> 1)",[["E_OPERAND",2]]],
  ["f(1","(call f 1)",[["E_CLOSE",3]]],
  ["1 $ + 2","(+ 1 2)",[["E_CHAR",2]]],
  ["1$2","1",[["E_CHAR",1],["E_EXTRA",2]]],
  ["a ? b","a",[["E_CHAR",2],["E_EXTRA",4]]],
  ["a ??? b","(?? a b)",[["E_CHAR",4]]],
  ["1. + .5","(+ 1 5)",[["E_CHAR",1],["E_CHAR",5]]],
  ["2 *** 3","(* (** 2 <error>) 3)",[["E_OPERAND",4]]],
  ["(1 (2 + ) 3) * 4","(* 1 4)",[["E_CLOSE",3]]],
  ["((1 2) 3","1",[["E_CLOSE",4],["E_CLOSE",7]]],
  ["(1 2) + (3 4) + 5 6","(+ (+ 1 3) 5)",[["E_CLOSE",3],["E_CLOSE",11],["E_EXTRA",18]]],
  ["f(g(1 2) + , 3) + ","(+ (call f (+ (call g 1) <error>) 3) <error>)",[["E_CLOSE",6],["E_OPERAND",11],["E_OPERAND",18]]],
  ["a ** ?? b","(?? (** a <error>) b)",[["E_OPERAND",5]]],
  ["1 + (2 * ) - (3 4) / @ 5 )","(- (+ 1 (* 2 <error>)) (/ 3 5))",[["E_OPERAND",9],["E_CLOSE",16],["E_CHAR",21],["E_EXTRA",25]]],
  ["x = f(1, 2","x",[["E_CHAR",2],["E_EXTRA",4]]],
  ["(a ?? ) ** (b","(** (?? a <error>) b)",[["E_OPERAND",6],["E_CLOSE",13]]],
  ["- - ","(neg (neg <error>))",[["E_OPERAND",4]]],
  ["f(1 + , 2 ** , 3)","(call f (+ 1 <error>) (** 2 <error>) 3)",[["E_OPERAND",6],["E_OPERAND",13]]],
  ["(1 + 2))","(+ 1 2)",[["E_EXTRA",7]]],
  ["a b c","a",[["E_EXTRA",2]]],
  ["f((1 2), (3","(call f 1 3)",[["E_CLOSE",5],["E_CLOSE",11]]],
  ["#(1","1",[["E_CHAR",0],["E_CLOSE",3]]],
  ["1 ** 2 ** ) ?? x","(** 1 (** 2 <error>))",[["E_OPERAND",10]]],
  ["1 2 $ ~","1",[["E_EXTRA",2],["E_CHAR",4],["E_CHAR",6]]],
  ["a\n+ \n","(+ a <error>)",[["E_OPERAND",5]]],
];

let failed = 0;

function fail(src, what) {
  failed++;
  console.log(`FAIL ${JSON.stringify(src)}: ${what}`);
}

for (const [src, tree, errors] of CASES) {
  let result;
  try {
    result = parse(src);
  } catch (e) {
    fail(src, `parse threw ${e}`);
    continue;
  }
  if (!result || !Array.isArray(result.errors) || !result.ast) {
    fail(src, 'parse did not return { ast, errors }');
    continue;
  }
  let shown;
  try {
    shown = show(result.ast);
  } catch (e) {
    fail(src, `bad tree: ${e.message}`);
    continue;
  }
  if (shown !== tree) fail(src, `tree is ${shown}, want ${tree}`);
  const got = JSON.stringify(result.errors.map((e) => [e.code, e.pos]));
  if (got !== JSON.stringify(errors)) fail(src, `errors are ${got}, want ${JSON.stringify(errors)}`);
}

// parse must not throw on anything, and must keep its promises about errors.
let seed = 12345;
function rand(n) {
  seed = (seed * 1103515245 + 12345) % 2147483648;
  return Math.floor(seed / 65536) % n;
}
const PIECES = ['1', '22', 'a', 'f(', '(', ')', ',', '+', '-', '*', '/', '**', '??', '?', '.', '$', ' ', '3.5'];
for (let i = 0; i < 300; i++) {
  let src = '';
  const n = rand(12);
  for (let j = 0; j < n; j++) src += PIECES[rand(PIECES.length)];
  try {
    const result = parse(src);
    show(result.ast);
    const positions = result.errors.map((e) => e.pos);
    for (let k = 1; k < positions.length; k++) {
      if (!(positions[k - 1] < positions[k])) throw new Error('errors not sorted by distinct pos');
    }
    for (const e of result.errors) {
      if (!['E_CHAR', 'E_OPERAND', 'E_CLOSE', 'E_EXTRA'].includes(e.code)) throw new Error(`bad code ${e.code}`);
      if (!Number.isInteger(e.pos) || e.pos < 0 || e.pos > src.length) throw new Error(`bad pos ${e.pos}`);
    }
    const invalid = [...src].filter((c, at) => c === '.' ? !(/[0-9]/.test(src[at - 1] || '') && /[0-9]/.test(src[at + 1] || '')) : c === '$').length;
    if (result.errors.filter((e) => e.code === 'E_CHAR').length < invalid) throw new Error('missing E_CHAR');
  } catch (e) {
    fail(src, `on generated input: ${e.message}`);
  }
}

if (failed) {
  console.log(`${failed} failed`);
  process.exit(1);
}
console.log('ok');
