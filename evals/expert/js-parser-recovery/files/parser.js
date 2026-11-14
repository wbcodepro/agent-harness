'use strict';

// A recursive-descent parser for a small expression language.
// See SPEC.md for what it is supposed to become.

function tokenize(src) {
  const tokens = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === ' ' || c === '\t' || c === '\n' || c === '\r') {
      i++;
    } else if (c >= '0' && c <= '9') {
      let j = i;
      while (j < src.length && src[j] >= '0' && src[j] <= '9') j++;
      if (src[j] === '.' && src[j + 1] >= '0' && src[j + 1] <= '9') {
        j++;
        while (j < src.length && src[j] >= '0' && src[j] <= '9') j++;
      }
      tokens.push({ kind: 'num', text: src.slice(i, j), pos: i });
      i = j;
    } else if (/[A-Za-z_]/.test(c)) {
      let j = i;
      while (j < src.length && /[A-Za-z0-9_]/.test(src[j])) j++;
      tokens.push({ kind: 'name', text: src.slice(i, j), pos: i });
      i = j;
    } else if ('+-*/(),'.includes(c)) {
      tokens.push({ kind: c, text: c, pos: i });
      i++;
    } else {
      throw new SyntaxError(`unexpected character ${JSON.stringify(c)} at ${i}`);
    }
  }
  tokens.push({ kind: 'end', text: '', pos: src.length });
  return tokens;
}

class Parser {
  constructor(tokens) {
    this.tokens = tokens;
    this.at = 0;
  }

  peek() {
    return this.tokens[this.at];
  }

  next() {
    return this.tokens[this.at++];
  }

  expect(kind) {
    const token = this.peek();
    if (token.kind !== kind) {
      throw new SyntaxError(`expected ${kind} at ${token.pos}`);
    }
    return this.next();
  }

  expr() {
    return this.additive();
  }

  additive() {
    let left = this.term();
    while (this.peek().kind === '+' || this.peek().kind === '-') {
      const op = this.next().kind;
      left = { type: 'bin', op, left, right: this.term() };
    }
    return left;
  }

  term() {
    let left = this.unary();
    while (this.peek().kind === '*' || this.peek().kind === '/') {
      const op = this.next().kind;
      left = { type: 'bin', op, left, right: this.unary() };
    }
    return left;
  }

  unary() {
    if (this.peek().kind === '-') {
      this.next();
      return { type: 'neg', arg: this.unary() };
    }
    return this.primary();
  }

  primary() {
    const token = this.peek();
    if (token.kind === 'num') {
      this.next();
      return { type: 'num', value: Number(token.text) };
    }
    if (token.kind === 'name') {
      this.next();
      if (this.peek().kind !== '(') return { type: 'var', name: token.text };
      this.next();
      const args = [];
      if (this.peek().kind !== ')') {
        args.push(this.expr());
        while (this.peek().kind === ',') {
          this.next();
          args.push(this.expr());
        }
      }
      this.expect(')');
      return { type: 'call', name: token.text, args };
    }
    if (token.kind === '(') {
      this.next();
      const inner = this.expr();
      this.expect(')');
      return inner;
    }
    throw new SyntaxError(`expected an operand at ${token.pos}`);
  }
}

// Parses `src` and returns the tree. Throws SyntaxError on the first problem.
function parse(src) {
  const parser = new Parser(tokenize(src));
  const ast = parser.expr();
  parser.expect('end');
  return ast;
}

// Renders a tree as an s-expression, for tests and debugging.
function show(node) {
  switch (node.type) {
    case 'num': return String(node.value);
    case 'var': return node.name;
    case 'neg': return `(neg ${show(node.arg)})`;
    case 'bin': return `(${node.op} ${show(node.left)} ${show(node.right)})`;
    case 'call': return `(call ${[node.name, ...node.args.map(show)].join(' ')})`;
    default: throw new Error(`unknown node type ${node.type}`);
  }
}

module.exports = { tokenize, parse, show };
