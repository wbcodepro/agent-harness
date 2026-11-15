"""hame browser helper: drives Chrome through Playwright for the `browser` tool.

The Rust side starts this file once per tool instance (`python3 -I` on a
temporary copy) and talks to it with line-delimited JSON on stdin/stdout:

    -> {"id": 7, "op": "observe", "tab": "t1", ...}
    <- {"id": 7, "ok": true, ...}
    <- {"id": 7, "ok": false, "error": {"code": "stale_ref", "message": "...",
                                         "effect": "none" | "done" | "unknown"}}

`effect` says whether a failed operation may have changed the page: `none`
(it failed before any input was emitted), `done` (the input was emitted and
then something else happened, such as a dialog opening) or `unknown`.

The helper holds the persistent browser contexts, the tabs and the element
handles behind refs. It never decides what is authorized: the Rust side
does, and also tells it which origins a profile may load so that navigation
anywhere else is blocked before a request leaves the machine.
"""

import asyncio
import base64
import hashlib
import json
import os
import shutil
import sys
import tempfile
import time
from urllib.parse import urljoin, urlsplit

PROTOCOL = 1
CONTRACT = 1
VERSION = "0.1.0"
OPERATIONS = [
    "open", "navigate", "back", "observe", "inspect", "act", "wait", "extract",
    "screenshot", "tabs", "dialog", "close", "check",
]

# The real stdout carries the protocol; anything printed by accident goes to
# stderr instead of corrupting it.
_OUT = open(sys.__stdout__.fileno(), "w", encoding="utf-8", newline="\n", closefd=False)
sys.stdout = sys.stderr


def send(obj):
    _OUT.write(json.dumps(obj, ensure_ascii=False) + "\n")
    _OUT.flush()


class OpError(Exception):
    def __init__(self, code, message, effect="none", **extra):
        super().__init__(message)
        self.code = code
        self.message = message
        self.effect = effect
        self.extra = extra


def glob_match(pattern, value):
    """`*` matches any run of characters; everything else literally, ignoring
    case. The same rule as `hame_session::contract::glob_match`."""
    p, v = pattern.lower(), value.lower()
    pi = vi = 0
    star, mark = -1, 0
    while vi < len(v):
        if pi < len(p) and p[pi] != "*" and p[pi] == v[vi]:
            pi += 1
            vi += 1
        elif pi < len(p) and p[pi] == "*":
            star, mark = pi, vi
            pi += 1
        elif star >= 0:
            pi = star + 1
            mark += 1
            vi = mark
        else:
            return False
    return all(c == "*" for c in p[pi:])


def origin_of(url):
    try:
        parts = urlsplit(url or "")
        port = parts.port
    except ValueError:
        return None
    if parts.scheme not in ("http", "https") or not parts.hostname:
        return None
    host = parts.hostname
    if ":" in host:
        host = f"[{host}]"
    default = 80 if parts.scheme == "http" else 443
    return f"{parts.scheme}://{host}" + (f":{port}" if port and port != default else "")


ACTIONABILITY = (
    "not visible", "not enabled", "not editable", "not stable", "intercepts pointer events",
    "detached", "outside of the viewport", "not attached", "not an <input>", "not a checkbox",
    "disabled", "hidden",
)


def pw_reason(error):
    """The useful line of a Playwright error: the last actionability step of
    its call log when there is one, else its first line."""
    lines = [line.strip() for line in str(error).splitlines() if line.strip()]
    log = [line[2:] for line in lines if line.startswith("- ")]
    hits = [line for line in log if any(k in line for k in ACTIONABILITY)]
    if hits:
        return hits[-1][:300]
    return (lines[0] if lines else str(error))[:300]


def is_actionability(error):
    text = str(error)
    return any(k in text for k in ACTIONABILITY)


# ---------------------------------------------------------------------------
# Page-side code. LIB is prepended to every script that needs it, so the
# page never holds hame's functions between calls.

LIB = r"""
const H = (() => {
  const norm = s => (s || '').replace(/\s+/g, ' ').trim();
  const cut = (s, n) => s.length > n ? s.slice(0, n - 1) + '…' : s;
  const ROLES = new Set(['button','link','checkbox','radio','tab','menuitem','menuitemcheckbox',
    'menuitemradio','option','switch','combobox','textbox','searchbox','slider','spinbutton',
    'treeitem','listbox','file']);
  const SKIP = new Set(['SCRIPT','STYLE','NOSCRIPT','TEMPLATE','HEAD','META','LINK','svg','SVG','OPTION','DATALIST']);
  function role(el) {
    const r = (el.getAttribute('role') || '').trim().split(/\s+/)[0].toLowerCase();
    if (r && r !== 'presentation' && r !== 'none' && r !== 'generic') return r;
    const tag = el.tagName.toLowerCase();
    switch (tag) {
      case 'a': return el.hasAttribute('href') ? 'link' : '';
      case 'button': case 'summary': return 'button';
      case 'select': return (el.multiple || el.size > 1) ? 'listbox' : 'combobox';
      case 'textarea': return 'textbox';
      case 'input': {
        const t = (el.getAttribute('type') || 'text').toLowerCase();
        if (t === 'hidden') return '';
        if (['button','submit','reset','image'].includes(t)) return 'button';
        if (t === 'checkbox' || t === 'radio') return t;
        if (t === 'range') return 'slider';
        if (t === 'number') return 'spinbutton';
        if (t === 'search') return 'searchbox';
        if (t === 'file') return 'file';
        return 'textbox';
      }
      case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': return 'heading';
      case 'table': return 'table';
      case 'ul': case 'ol': return 'list';
      case 'dialog': return 'dialog';
      case 'img': return 'img';
      case 'legend': return 'legend';
    }
    if (el.getAttribute('contenteditable') !== null && el.isContentEditable) return 'textbox';
    return '';
  }
  function textOf(el) { return norm(el.innerText !== undefined ? el.innerText : el.textContent); }
  function labelText(label) {
    let out = '';
    const walk = n => {
      for (const c of n.childNodes) {
        if (c.nodeType === 3) out += c.textContent + ' ';
        else if (c.nodeType === 1 && !['INPUT','SELECT','TEXTAREA','BUTTON'].includes(c.tagName)) walk(c);
      }
    };
    walk(label);
    return norm(out);
  }
  function name(el) {
    const lb = el.getAttribute('aria-labelledby');
    if (lb) {
      const t = norm(lb.split(/\s+/).map(id => { const n = document.getElementById(id); return n ? textOf(n) : ''; }).join(' '));
      if (t) return cut(t, 100);
    }
    const al = norm(el.getAttribute('aria-label'));
    if (al) return cut(al, 100);
    const tag = el.tagName.toLowerCase();
    const type = (el.getAttribute('type') || '').toLowerCase();
    if (tag === 'input' && ['submit','button','reset'].includes(type))
      return cut(norm(el.value) || (type === 'submit' ? 'Submit' : type === 'reset' ? 'Reset' : ''), 100);
    if (tag === 'input' && type === 'image') return cut(norm(el.alt) || 'Submit', 100);
    if (tag === 'input' || tag === 'select' || tag === 'textarea') {
      if (el.labels && el.labels.length) {
        const t = norm(Array.from(el.labels).map(labelText).join(' '));
        if (t) return cut(t, 100);
      }
      return cut(norm(el.getAttribute('placeholder')) || norm(el.getAttribute('title')), 100);
    }
    if (tag === 'img') return cut(norm(el.alt), 100);
    if (tag === 'table') { const c = el.querySelector('caption'); return c ? cut(textOf(c), 100) : ''; }
    if (tag === 'ul' || tag === 'ol') return '';
    const r = role(el);
    if (tag === 'dialog' || r === 'dialog' || r === 'alertdialog') {
      const h = el.querySelector('h1,h2,h3,h4,h5,h6'); return h ? cut(textOf(h), 100) : '';
    }
    const t = textOf(el);
    if (t) return cut(t, 100);
    return cut(norm(el.getAttribute('title')), 100);
  }
  function context(el) {
    for (let a = el.parentElement; a; a = a.parentElement) {
      const tag = a.tagName.toLowerCase();
      if (tag === 'fieldset') {
        const lg = a.querySelector(':scope > legend');
        if (lg) return cut(textOf(lg), 40);
      }
      if (tag === 'tr') {
        const first = a.querySelector(':scope > th, :scope > td');
        if (first && !first.contains(el)) return cut(textOf(first), 40);
      }
      const ar = (a.getAttribute('role') || '').toLowerCase();
      if (tag === 'dialog' || ar === 'dialog' || ar === 'alertdialog') {
        const n = name(a); if (n) return cut(n, 40);
      }
      const al = norm(a.getAttribute('aria-label'));
      if (al && (['form','section','nav','aside'].includes(tag) || ['region','group','form','navigation'].includes(ar)))
        return cut(al, 40);
    }
    const h = document.evaluate(
      'preceding::*[self::h1 or self::h2 or self::h3 or self::h4 or self::h5 or self::h6][1]',
      el, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue;
    return h ? cut(textOf(h), 40) : '';
  }
  function fingerprint(el) { return {role: role(el), name: name(el), ctx: context(el)}; }
  function hasBox(el) { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; }
  function visible(el) {
    if (!el.isConnected) return false;
    if (typeof el.checkVisibility === 'function' && !el.checkVisibility({visibilityProperty: true})) return false;
    return hasBox(el);
  }
  function interactive(el, r) {
    if (ROLES.has(r)) return true;
    const tag = el.tagName;
    if (tag === 'A' && el.hasAttribute('href')) return true;
    if (['BUTTON','SELECT','TEXTAREA','SUMMARY'].includes(tag)) return true;
    if (tag === 'INPUT' && (el.getAttribute('type') || '').toLowerCase() !== 'hidden') return true;
    if (el.hasAttribute('onclick') && el.tabIndex >= 0) return true;
    return false;
  }
  function isSubmit(el) {
    const tag = el.tagName, type = (el.getAttribute('type') || '').toLowerCase();
    return (tag === 'BUTTON' && (type === '' || type === 'submit')) || (tag === 'INPUT' && (type === 'submit' || type === 'image'));
  }
  function method(el) {
    const form = el.form || el.closest('form');
    return ((isSubmit(el) && el.getAttribute('formmethod')) || (form && form.getAttribute('method')) || '').toLowerCase();
  }
  function flags(el, r) {
    const f = [];
    const tag = el.tagName, type = (el.getAttribute('type') || '').toLowerCase();
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') f.push('disabled');
    if (r === 'checkbox' || r === 'radio' || r === 'switch') {
      const c = el.checked !== undefined ? el.checked : el.getAttribute('aria-checked') === 'true';
      f.push(c ? 'checked' : 'unchecked');
    }
    if (el.getAttribute('aria-expanded')) f.push(el.getAttribute('aria-expanded') === 'true' ? 'expanded' : 'collapsed');
    if (el.getAttribute('aria-selected') === 'true') f.push('selected');
    if (el.required || el.getAttribute('aria-required') === 'true') f.push('required');
    if (el.getAttribute('aria-invalid') === 'true') f.push('invalid');
    if (el.readOnly) f.push('readonly');
    if (tag === 'INPUT' && ['date','time','datetime-local','month','week','email','tel','url','number','password'].includes(type)) f.push('type=' + type);
    if ((el.form || el.closest('form')) && isSubmit(el)) f.push(method(el) === 'get' ? 'submit(get)' : 'submit');
    return f;
  }
  function value(el, r) {
    const tag = el.tagName, type = (el.getAttribute('type') || '').toLowerCase();
    if (tag === 'SELECT') return Array.from(el.selectedOptions).map(o => norm(o.label || o.text)).join(', ');
    if (tag === 'TEXTAREA' || (tag === 'INPUT' && !['checkbox','radio','button','submit','reset','image','file'].includes(type)))
      return type === 'password' ? (el.value ? '•••' : '') : cut(el.value, 80);
    if (tag === 'INPUT' && type === 'file') return Array.from(el.files || []).map(f => f.name).join(', ');
    if (r === 'textbox' && el.isContentEditable) return cut(textOf(el), 80);
    return null;
  }
  return {norm, cut, role, name, context, fingerprint, visible, interactive, isSubmit, method, flags, value, textOf, SKIP, hasBox};
})();
"""

COLLECT = LIB + r"""
(args) => {
  const known = args.known || [];
  let root = args.scopeEl || null;
  if (!root && args.scopeSel) root = document.querySelector(args.scopeSel);
  if (!root) root = args.scopeSel ? null : document.body;
  if (!root) return {items: [], els: [], missing: true, visited: 0};
  const items = [], els = [];
  let visited = 0, truncated = false;
  const IMPORTANT = /\b(total|subtotal|balance|amount due|error|failed|invalid|warning|signed in|logged in|account|success|saved|thank)/i;
  const addEl = el => { els.push(el); return els.length - 1; };
  const visit = (el, inInter, textDone) => {
    if (visited++ > 20000) { truncated = true; return; }
    if (H.SKIP.has(el.tagName)) return;
    if (el.hidden || el.getAttribute('aria-hidden') === 'true') return;
    const st = getComputedStyle(el);
    if (st.display === 'none') return;
    const vis = st.visibility !== 'hidden' && st.visibility !== 'collapse' && H.hasBox(el);
    const r = H.role(el);
    const tag = el.tagName;
    let childInter = inInter, childText = textDone;
    if (vis && !inInter && H.interactive(el, r)) {
      const fp = H.fingerprint(el);
      const item = {k: 'el', role: r || tag.toLowerCase(), name: fp.name, ctx: fp.ctx, fp,
                    flags: H.flags(el, r), value: H.value(el, r), el: addEl(el), known: known.indexOf(el)};
      const link = tag === 'A' ? el.href : null;
      if (link && !link.startsWith(location.origin)) item.href = link;
      if (tag === 'SELECT') item.options = Array.from(el.options).slice(0, 12).map(o => H.norm(o.label || o.text));
      items.push(item);
      childInter = true; childText = true;
    } else if (vis && r === 'heading') {
      items.push({k: 'heading', role: 'heading', name: H.textOf(el).slice(0, 150), level: tag.toLowerCase()});
      childText = true;
    } else if (vis && r === 'legend') {
      items.push({k: 'heading', role: 'group', name: H.textOf(el).slice(0, 100), level: 'legend'});
      childText = true;
    } else if ((r === 'dialog' || r === 'alertdialog') && vis) {
      items.push({k: 'dialog', role: r, name: H.name(el), fp: H.fingerprint(el), el: addEl(el), known: known.indexOf(el),
                  modal: el.matches('dialog:modal') || el.getAttribute('aria-modal') === 'true'});
    } else if (tag === 'TABLE' && vis) {
      const rows = Array.from(el.rows).filter(x => x.checkVisibility ? x.checkVisibility() : true);
      const head = el.tHead && el.tHead.rows.length ? el.tHead.rows[el.tHead.rows.length - 1] :
        (rows.length && Array.from(rows[0].cells).every(c => c.tagName === 'TH') ? rows[0] : null);
      const cols = head ? Array.from(head.cells).map(c => H.textOf(c).slice(0, 40)) : [];
      const body = rows.filter(x => x !== head && !(el.tHead && el.tHead.contains(x)));
      const sample = body.slice(0, 3).map(x => Array.from(x.cells).map(c => H.textOf(c).slice(0, 40)));
      items.push({k: 'table', role: 'table', name: H.name(el), fp: H.fingerprint(el), cols, rows: body.length, sample,
                  el: addEl(el), known: known.indexOf(el)});
      childText = true;
    } else if ((tag === 'UL' || tag === 'OL') && vis && el.children.length >= 3 && !inInter) {
      items.push({k: 'list', role: 'list', name: H.name(el), fp: H.fingerprint(el), count: Array.from(el.children).filter(c => c.tagName === 'LI').length,
                  el: addEl(el), known: known.indexOf(el)});
    } else if (vis && !inInter && !textDone) {
      const ar = (el.getAttribute('role') || '').toLowerCase();
      const live = el.getAttribute('aria-live');
      const cls = (typeof el.className === 'string' ? el.className : '') + ' ' + (el.id || '');
      if (ar === 'alert' || ar === 'status' || (live && live !== 'off') || /\b(error|alert|danger|warning|invalid)\b/i.test(cls)) {
        const t = H.textOf(el);
        if (t) items.push({k: 'alert', role: ar || 'alert', name: t.slice(0, 200)});
        childText = true;
      } else if (tag === 'LABEL') {
        childText = true;
      } else {
        let own = false;
        for (const c of el.childNodes) if (c.nodeType === 3 && c.textContent.trim()) { own = true; break; }
        if (own) {
          const t = H.textOf(el);
          if (t) items.push({k: 'text', role: 'text', name: t.slice(0, 200), important: IMPORTANT.test(t)});
          childText = true;
        }
      }
    }
    for (const c of el.children) visit(c, childInter, childText);
    if (el.shadowRoot) for (const c of el.shadowRoot.children) visit(c, childInter, childText);
  };
  visit(root, false, false);
  return {items, els, truncated, visited, missing: false};
}
"""

FIND = LIB + r"""
(args) => {
  const want = H.norm(args.text).toLowerCase();
  const known = args.known || [];
  const sel = 'a[href],button,input,select,textarea,summary,[role],[contenteditable],[onclick]';
  const out = [], els = [];
  for (const el of document.querySelectorAll(sel)) {
    const r = H.role(el);
    if (!H.interactive(el, r) || !H.visible(el)) continue;
    if (args.role && r !== args.role) continue;
    if (H.name(el).toLowerCase() !== want) continue;
    els.push(el);
    const fp = H.fingerprint(el);
    out.push({role: r, name: fp.name, ctx: fp.ctx, fp, el: els.length - 1, known: known.indexOf(el)});
  }
  return {items: out, els, truncated: false, missing: false};
}
"""

VALIDATE = LIB + r"""
(el, fp) => {
  if (!el.isConnected) return 'was removed from the page';
  const now = H.fingerprint(el);
  for (const k of ['role', 'name', 'ctx'])
    if (now[k] !== fp[k]) return `changed (${k} was "${fp[k]}", now "${now[k]}")`;
  return null;
}
"""

FACTS = LIB + r"""
(el, key) => {
  const tag = el.tagName.toLowerCase(), type = (el.getAttribute('type') || '').toLowerCase();
  const form = el.form || el.closest('form');
  const textLike = tag === 'input' && !['checkbox','radio','button','submit','reset','image','file','hidden','range','color'].includes(type);
  let submits = false;
  if (form && H.isSubmit(el)) submits = true;
  if (form && key === 'Enter' && textLike) submits = true;
  const action = form ? ((H.isSubmit(el) && el.getAttribute('formaction') ? el.formAction : form.action) || location.href) : null;
  const fields = [];
  const TOKEN = /csrf|xsrf|token|nonce|authenticity|captcha/i;
  if (form) for (const f of form.elements) {
    const n = f.name; if (!n) continue;
    const ft = (f.type || '').toLowerCase();
    if (['submit','button','reset','image'].includes(ft)) continue;
    if (ft === 'hidden' && TOKEN.test(n)) continue;
    if ((ft === 'checkbox' || ft === 'radio') && !f.checked) continue;
    if (ft === 'file') { fields.push(n + '=' + Array.from(f.files || []).map(x => x.name + ':' + x.size).join(',')); continue; }
    if (f.tagName === 'SELECT' && f.multiple) { fields.push(n + '=' + Array.from(f.selectedOptions).map(o => o.value).join(',')); continue; }
    fields.push(n + '=' + f.value);
  }
  fields.sort();
  const link = el.closest('a[href]');
  return {
    tag, type, submits, method: H.method(el), action,
    form: form ? (form.id || form.getAttribute('name') || ('form#' + Array.from(document.forms).indexOf(form))) : null,
    fields,
    control: (el.getAttribute('name') || '') + '|' + H.role(el) + '|' + H.name(el) + '|' + H.context(el),
    href: link ? link.href : null,
    file_input: tag === 'input' && type === 'file',
    password: type === 'password',
    select: tag === 'select',
    options: tag === 'select' ? Array.from(el.options).map(o => ({label: H.norm(o.label || o.text), value: o.value})) : null,
    role: H.role(el), name: H.name(el),
  };
}
"""

EXTRACT = LIB + r"""
(root) => {
  const t = root.tagName === 'TABLE' ? root : root.querySelector('table');
  if (t) {
    const rows = Array.from(t.rows);
    let head = null;
    if (t.tHead && t.tHead.rows.length) head = t.tHead.rows[t.tHead.rows.length - 1];
    else if (rows.length && Array.from(rows[0].cells).every(c => c.tagName === 'TH')) head = rows[0];
    const cols = head ? Array.from(head.cells).map(c => H.textOf(c)) : [];
    const data = [];
    for (const r of rows) {
      if (r === head || (t.tHead && t.tHead.contains(r)) || (t.tFoot && t.tFoot.contains(r))) continue;
      if (r.checkVisibility && !r.checkVisibility()) continue;
      const cells = Array.from(r.cells).map(c => H.textOf(c));
      if (cells.every(c => !c)) continue;
      data.push(cells);
    }
    return {kind: 'table', cols, rows: data, text: H.textOf(t).slice(0, 4000)};
  }
  const list = root.matches('ul,ol') ? root : root.querySelector('ul,ol');
  if (list) {
    const data = Array.from(list.children).filter(li => li.tagName === 'LI' && (!li.checkVisibility || li.checkVisibility()))
      .map(li => [H.textOf(li)]);
    return {kind: 'list', cols: ['text'], rows: data, text: H.textOf(list).slice(0, 4000)};
  }
  return null;
}
"""

FIND_TABLE = r"""
(cols) => {
  const norm = s => (s || '').replace(/\s+/g, ' ').trim();
  const out = [];
  for (const t of document.querySelectorAll('table')) {
    const rows = Array.from(t.rows);
    let head = null;
    if (t.tHead && t.tHead.rows.length) head = t.tHead.rows[t.tHead.rows.length - 1];
    else if (rows.length && Array.from(rows[0].cells).every(c => c.tagName === 'TH')) head = rows[0];
    const hc = head ? Array.from(head.cells).map(c => norm(c.innerText)) : [];
    if (JSON.stringify(hc) === JSON.stringify(cols)) out.push(t);
  }
  return out.length === 1 ? out[0] : out.length;
}
"""


MODALS = LIB + r"""
() => Array.from(document.querySelectorAll('dialog[open], [aria-modal="true"], [role="alertdialog"]'))
  .filter(d => H.visible(d) && (d.tagName !== 'DIALOG' || d.matches(':modal') || d.getAttribute('aria-modal') === 'true'))
  .map(d => H.name(d) || d.tagName.toLowerCase())
"""


class Tab:
    def __init__(self, helper, tid, profile, page):
        self.id = tid
        self.profile = profile
        self.page = page
        self.generation = helper.next_gen()
        self.known_handles = []
        self.known_refs = []
        self.nav_count = 0
        self.pending = set()
        self.nav_idle = asyncio.Event()
        self.nav_idle.set()
        self.last_status = None
        self.dialog = None
        self.dialog_event = asyncio.Event()
        self.stalled = None
        self.downloads = []
        self.download_event = asyncio.Event()
        self.consumed = 0
        self.closed = False
        self.busy = False
        self.guarded = False


class Ref:
    def __init__(self, rid, tab, handle, fp, obs):
        self.id = rid
        self.tab = tab.id
        self.generation = tab.generation
        self.handle = handle
        self.fp = fp
        self.obs = obs


class Helper:
    def __init__(self):
        self.pw = None
        self.contexts = {}
        self.spare = {}
        self.tabs = {}
        self.page_tabs = {}
        self.refs = {}
        self.gen = 0
        self.ref_counter = 0
        self.tab_counter = 0
        self.obs_counter = 0
        self.events = []
        self.blocked = []
        self.allow = []
        self.root = None
        self.channel = "chrome"
        self.executable = None
        self.headless = True
        self.download_dir = tempfile.mkdtemp(prefix="hame-browser-dl-")
        self.opening = False

    def next_gen(self):
        self.gen += 1
        return self.gen

    # -- profiles and tabs ---------------------------------------------------

    def allowed(self, profile, url):
        scheme = urlsplit(url).scheme.lower() if url else ""
        if scheme in ("about", "data", "blob"):
            return True
        origin = origin_of(url)
        if origin is None:
            return False
        return any(glob_match(p, profile) and glob_match(o, origin) for p, o in self.allow)

    async def guard(self, profile, route):
        request = route.request
        try:
            if request.is_navigation_request() and not self.allowed(profile, request.url):
                self.blocked.append(request.url)
                self.events.append(f"blocked navigation to {request.url} (origin not granted for profile {profile})")
                # `aborted` leaves the page where it was instead of
                # committing an error page.
                await route.abort("aborted")
                return
            await route.continue_()
        except Exception:
            pass

    async def attach_redirect_guard(self, tab):
        """Playwright hands a route only the first URL of a redirect chain.
        This checks every redirect a document request gets, at the response,
        before the browser follows it."""
        if tab.guarded:
            return
        tab.guarded = True
        try:
            cdp = await self.contexts[tab.profile].new_cdp_session(tab.page)
            cdp.on("Fetch.requestPaused", lambda ev: asyncio.ensure_future(self.on_paused(tab, cdp, ev)))
            await cdp.send("Fetch.enable", {"patterns": [
                {"urlPattern": "*", "resourceType": "Document", "requestStage": "Response"}]})
        except Exception as e:
            self.events.append(f"the redirect guard is not active in {tab.id}: {pw_reason(e)}")

    async def on_paused(self, tab, cdp, ev):
        rid = ev["requestId"]
        try:
            status = ev.get("responseStatusCode") or 0
            if 300 <= status < 400:
                location = next((h["value"] for h in ev.get("responseHeaders", [])
                                 if h["name"].lower() == "location"), None)
                if location:
                    target = urljoin(ev["request"]["url"], location)
                    if not self.allowed(tab.profile, target):
                        self.blocked.append(target)
                        self.events.append(f"blocked a redirect to {target} (origin not granted for "
                                           f"profile {tab.profile})")
                        await cdp.send("Fetch.failRequest", {"requestId": rid, "errorReason": "Aborted"})
                        return
            await cdp.send("Fetch.continueRequest", {"requestId": rid})
        except Exception:
            pass

    def clear_stale_lock(self, directory):
        lock = os.path.join(directory, "SingletonLock")
        try:
            target = os.readlink(lock)
        except OSError:
            return
        try:
            pid = int(target.rsplit("-", 1)[-1])
            os.kill(pid, 0)
            return
        except (ValueError, ProcessLookupError):
            pass
        except PermissionError:
            return
        for name in ("SingletonLock", "SingletonSocket", "SingletonCookie"):
            try:
                os.unlink(os.path.join(directory, name))
            except OSError:
                pass

    async def context(self, profile):
        if profile in self.contexts:
            return self.contexts[profile]
        directory = os.path.join(self.root, profile)
        os.makedirs(directory, exist_ok=True)
        kwargs = dict(
            headless=self.headless,
            accept_downloads=True,
            downloads_path=self.download_dir,
            viewport={"width": 1280, "height": 900},
            args=["--no-first-run", "--no-default-browser-check", "--use-mock-keychain",
                  "--password-store=basic"],
        )
        if self.executable:
            kwargs["executable_path"] = self.executable
        elif self.channel and self.channel != "chromium":
            kwargs["channel"] = self.channel
        deadline = time.monotonic() + 15
        while True:
            self.clear_stale_lock(directory)
            try:
                ctx = await self.pw.chromium.launch_persistent_context(directory, **kwargs)
                break
            except Exception as e:
                text = str(e)
                # A browser from a crashed helper may still hold the profile
                # for a moment while it shuts down.
                if ("already in use" in text or "ProcessSingleton" in text or "SingletonLock" in text) \
                        and time.monotonic() < deadline:
                    await asyncio.sleep(0.25)
                    continue
                raise OpError("helper_unavailable", f"cannot start the browser for profile {profile}: {pw_reason(e)}")
        self.contexts[profile] = ctx
        ctx.on("page", lambda page: self.adopt(profile, page, opened=True))
        await ctx.route("**/*", lambda route: self.guard(profile, route))
        pages = list(ctx.pages)
        if pages:
            self.spare[profile] = pages[0]
        return ctx

    def adopt(self, profile, page, opened=False):
        key = id(page)
        if key in self.page_tabs:
            return self.page_tabs[key]
        if opened and self.spare.get(profile) is page:
            return None
        if self.opening:
            opened = False
        self.tab_counter += 1
        tab = Tab(self, f"t{self.tab_counter}", profile, page)
        self.tabs[tab.id] = tab
        self.page_tabs[key] = tab

        def on_nav(frame):
            if frame.parent_frame is None:
                tab.generation = self.next_gen()
                tab.known_handles = []
                tab.known_refs = []

        def on_request(request):
            try:
                if request.is_navigation_request() and request.frame.parent_frame is None:
                    tab.pending.add(request)
                    tab.nav_count += 1
                    tab.nav_idle.clear()
            except Exception:
                pass

        def on_request_done(request):
            if request in tab.pending:
                tab.pending.discard(request)
                if not tab.pending:
                    tab.nav_idle.set()

        def on_response(response):
            try:
                if response.request.is_navigation_request() and response.frame.parent_frame is None:
                    tab.last_status = response.status
            except Exception:
                pass

        def on_dialog(dialog):
            tab.dialog = dialog
            tab.dialog_event.set()
            if not tab.busy:
                self.events.append(f"a native {dialog.type} dialog opened in {tab.id}: {dialog.message[:200]!r}")

        def on_download(download):
            tab.downloads.append(download)
            tab.download_event.set()

        def on_close(_page=None):
            tab.closed = True

        page.on("framenavigated", on_nav)
        page.on("request", on_request)
        page.on("requestfinished", on_request_done)
        page.on("requestfailed", on_request_done)
        page.on("response", on_response)
        page.on("dialog", on_dialog)
        page.on("download", on_download)
        page.on("close", on_close)
        if opened:
            self.events.append(f"new tab {tab.id} opened in profile {profile} (by the page)")
            asyncio.ensure_future(self.attach_redirect_guard(tab))
        return tab

    def tab(self, req):
        tid = req.get("tab")
        tab = self.tabs.get(tid)
        if tab is None or tab.closed:
            raise OpError("not_found", f"There is no open tab {tid}.")
        expected = req.get("origin", "__unset__")
        if expected != "__unset__":
            current = origin_of(tab.page.url)
            if current != expected:
                raise OpError("origin_changed", f"the tab is now at {tab.page.url}",
                              origin=current, url=tab.page.url)
        return tab

    async def tab_info(self, tab):
        try:
            title = await asyncio.wait_for(tab.page.title(), 5)
        except Exception:
            title = ""
        return {"tab": tab.id, "profile": tab.profile, "url": tab.page.url, "origin": origin_of(tab.page.url),
                "title": title, "generation": tab.generation}

    # -- guarded execution -----------------------------------------------------

    def dialog_error(self, tab, during):
        d = tab.dialog
        what = f"a native {d.type} dialog is open: {d.message[:300]!r}"
        if during:
            what = f"the action opened a native {d.type} dialog: {d.message[:300]!r}"
        return OpError("dialog_open", what, effect="done" if during else "none",
                       dialog={"type": d.type, "message": d.message[:500]})

    async def guarded(self, tab, coro, seconds):
        if tab.dialog is not None:
            coro.close()
            raise self.dialog_error(tab, during=False)
        tab.busy = True
        tab.dialog_event.clear()
        task = asyncio.ensure_future(coro)
        waiter = asyncio.ensure_future(tab.dialog_event.wait())
        try:
            done, _ = await asyncio.wait({task, waiter}, timeout=seconds, return_when=asyncio.FIRST_COMPLETED)
        finally:
            tab.busy = False
            if not waiter.done():
                waiter.cancel()
        if task in done:
            return task.result()
        if tab.dialog is not None:
            tab.stalled = task
            task.add_done_callback(lambda t: t.cancelled() or t.exception())
            raise self.dialog_error(tab, during=True)
        task.cancel()
        raise OpError("timeout", f"the operation did not finish within {seconds:.0f} s", effect="unknown")

    async def settle(self, tab, nav_before, gen_before, timeout_ms):
        """Waits for a navigation the action started to finish loading."""
        await asyncio.sleep(0)
        if tab.nav_count != nav_before:
            try:
                await asyncio.wait_for(tab.nav_idle.wait(), timeout_ms / 1000)
            except asyncio.TimeoutError:
                raise OpError("timeout", "a navigation the action started did not finish", effect="unknown")
            if tab.generation != gen_before:
                try:
                    await tab.page.wait_for_load_state("load", timeout=timeout_ms)
                except Exception as e:
                    raise OpError("timeout", f"the new page did not finish loading: {pw_reason(e)}", effect="unknown")
        return tab.generation != gen_before

    # -- refs -----------------------------------------------------------------

    async def collect(self, tab, script, args):
        """Runs a collecting script; returns its items, the element handles
        they point to, and its other result fields."""
        args = dict(args)
        args["known"] = tab.known_handles
        result = await tab.page.evaluate_handle(script, args)
        try:
            meta = await (await result.get_property("items")).json_value()
            els_handle = await result.get_property("els")
            props = await els_handle.get_properties()
            handles = [props[str(i)].as_element() for i in range(len(props))]
            flags = {}
            for key in ("truncated", "missing"):
                flags[key] = await (await result.get_property(key)).json_value()
        finally:
            await result.dispose()
        return meta, handles, flags

    def bind(self, tab, meta, handles):
        """Gives each element item a ref: the one the same element already
        had in this document, a new one otherwise. Returns the observation
        id and the ref of each item (None for items without an element)."""
        self.obs_counter += 1
        obs = f"o{self.obs_counter}"
        ids = []
        for item in meta:
            idx = item.get("el", -1)
            if idx is None or idx < 0:
                ids.append(None)
                continue
            k = item.get("known", -1)
            if k is not None and 0 <= k < len(tab.known_refs):
                rid = tab.known_refs[k]
            else:
                self.ref_counter += 1
                rid = f"e{self.ref_counter}"
                tab.known_handles.append(handles[idx])
                tab.known_refs.append(rid)
            self.refs[rid] = Ref(rid, tab, handles[idx], item["fp"], obs)
            ids.append(rid)
        if len(self.refs) > 5000:
            for rid in [r for r, v in self.refs.items()
                        if v.tab not in self.tabs or v.generation != self.tabs[v.tab].generation]:
                del self.refs[rid]
        return obs, ids

    async def resolve(self, req, tab):
        rid = req.get("ref")
        if rid:
            ref = self.refs.get(rid)
            if ref is None:
                raise OpError("stale_ref", f"{rid} is not a ref this browser knows; it may come from before a restart")
            if ref.tab != tab.id:
                raise OpError("target_mismatch", f"{rid} belongs to tab {ref.tab}, not {tab.id}")
            if ref.generation != tab.generation:
                raise OpError("stale_ref", f"{rid} was observed in {ref.obs} before the page navigated or reloaded")
            try:
                problem = await asyncio.wait_for(ref.handle.evaluate(VALIDATE, ref.fp), 10)
            except asyncio.TimeoutError:
                raise OpError("timeout", f"could not check {rid}; the page is not responding")
            except Exception:
                problem = "is no longer attached to the page"
            if problem:
                raise OpError("stale_ref", f"{rid} ({ref.fp['role']} \"{ref.fp['name']}\") {problem} since observation {ref.obs}")
            return ref
        text = req.get("text")
        if text:
            meta, handles, _ = await self.collect(tab, FIND, {"text": text, "role": req.get("role")})
            if not meta:
                raise OpError("not_found", f"no visible interactive element is named {text!r}"
                              + (f" with role {req.get('role')}" if req.get("role") else ""))
            obs, ids = self.bind(tab, meta, handles)
            if len(meta) > 1:
                cands = [{"ref": rid, "role": m["role"], "name": m["name"], "ctx": m.get("ctx", "")}
                         for m, rid in zip(meta, ids)]
                raise OpError("ambiguous", f"{len(meta)} elements are named {text!r}", candidates=cands, obs=obs)
            return self.refs[ids[0]]
        if req.get("focused"):
            handle = (await tab.page.evaluate_handle("() => document.activeElement && document.activeElement !== document.body ? document.activeElement : null")).as_element()
            if handle is None:
                return None
            fp = await handle.evaluate(LIB + "e => H.fingerprint(e)")
            meta = [{"el": 0, "known": -1, "fp": fp}]
            obs, ids = self.bind(tab, meta, [handle])
            return self.refs[ids[0]]
        raise OpError("invalid_input", "give a ref (from observe) or text naming the element")

    # -- operations -----------------------------------------------------------

    async def op_hello(self, req):
        from playwright.async_api import async_playwright
        from importlib.metadata import version
        self.root = req["profile_root"]
        self.channel = req.get("channel") or "chrome"
        self.executable = req.get("executable")
        self.headless = req.get("headless", True)
        self.gen = int(req.get("generation_base", 0))
        # Ids continue past those of a crashed predecessor, so an old ref or
        # tab id can never name something new.
        self.ref_counter, self.tab_counter, self.obs_counter = (int(x) for x in req.get("id_base", [0, 0, 0]))
        self.allow = [list(x) for x in req.get("allow", [])]
        if req.get("protocol") != PROTOCOL:
            raise OpError("protocol", f"helper speaks protocol {PROTOCOL}, caller wants {req.get('protocol')}")
        self.pw = await async_playwright().start()
        return {
            "manifest": {
                "name": "hame-browser-playwright",
                "version": VERSION,
                "protocol": PROTOCOL,
                "channel": "browser",
                "operations": OPERATIONS,
                "platforms": ["macos", "linux", "windows"],
                "cancellable": False,
                "nested_usage": "none",
            },
            "contract": CONTRACT,
            "playwright": version("playwright"),
        }

    async def op_configure(self, req):
        self.allow = [list(x) for x in req.get("allow", [])]
        return {}

    async def op_open(self, req):
        profile = req["profile"]
        url = req["url"]
        if not self.allowed(profile, url):
            raise OpError("unauthorized", f"{url} is outside the origins granted to profile {profile}")
        ctx = await self.context(profile)
        page = self.spare.pop(profile, None)
        if page is None or page.is_closed():
            self.opening = True
            try:
                page = await ctx.new_page()
            finally:
                self.opening = False
        tab = self.adopt(profile, page)
        await self.attach_redirect_guard(tab)
        try:
            await self.goto(tab, url, req.get("timeout_ms", 30000))
        except OpError:
            # The caller never learns this tab's id, so it must not linger.
            await self.op_close({"tab": tab.id})
            raise
        return await self.tab_info(tab)

    async def goto(self, tab, url, timeout_ms):
        blocked_before = len(self.blocked)

        def blocked():
            urls = self.blocked[blocked_before:]
            return OpError("unauthorized", f"loading {url} led to {', '.join(urls)}, which was blocked: "
                                           "origin not granted")
        try:
            response = await self.guarded(tab, tab.page.goto(url, wait_until="load", timeout=timeout_ms),
                                          timeout_ms / 1000 + 5)
        except OpError:
            raise
        except Exception as e:
            if len(self.blocked) > blocked_before:
                raise blocked()
            if "Timeout" in type(e).__name__:
                raise OpError("timeout", f"{url} did not finish loading: {pw_reason(e)}")
            raise OpError("navigation_failed", f"cannot load {url}: {pw_reason(e)}")
        if len(self.blocked) > blocked_before:
            raise blocked()
        return response

    async def op_navigate(self, req):
        tab = self.tab(req)
        if not self.allowed(tab.profile, req["url"]):
            raise OpError("unauthorized", f"{req['url']} is outside the origins granted to profile {tab.profile}")
        await self.goto(tab, req["url"], req.get("timeout_ms", 30000))
        return await self.tab_info(tab)

    async def op_back(self, req):
        tab = self.tab(req)
        t = req.get("timeout_ms", 30000)
        try:
            response = await self.guarded(tab, tab.page.go_back(wait_until="load", timeout=t), t / 1000 + 5)
        except OpError:
            raise
        except Exception as e:
            raise OpError("navigation_failed", pw_reason(e))
        info = await self.tab_info(tab)
        info["moved"] = response is not None
        return info

    async def op_observe(self, req):
        tab = self.tab(req)
        args = {"scopeSel": None, "scopeEl": None}
        scope = req.get("scope")
        if scope:
            ref = self.refs.get(scope)
            if ref is not None:
                ref = await self.resolve({"ref": scope}, tab)
                args["scopeEl"] = ref.handle
            else:
                args["scopeSel"] = scope
        t = req.get("timeout_ms", 15000)
        meta, handles, flags = await self.guarded(tab, self.collect(tab, COLLECT, args), t / 1000)
        if flags.get("missing"):
            raise OpError("not_found", f"nothing on the page matches the scope {scope!r}")
        obs, ids = self.bind(tab, meta, handles)
        max_nodes = max(5, min(int(req.get("max_nodes", 120)), 1000))
        text, refs, complete, shown = self.render(tab, meta, ids, max_nodes, bool(flags.get("truncated")))
        info = await self.tab_info(tab)
        info.update(obs=obs, text=text, refs=refs, complete=complete, nodes=len(meta), shown=shown)
        return info

    def render(self, tab, meta, ids, max_nodes, truncated):
        def priority(m):
            k = m["k"]
            if k in ("dialog", "alert"):
                return 0
            if k == "heading" or (k == "text" and m.get("important")):
                return 1
            if k in ("el", "table", "list"):
                return 2
            return 3
        order = sorted(range(len(meta)), key=lambda i: (priority(meta[i]), i))
        keep = set(order[:max_nodes])
        complete = len(keep) == len(meta) and not truncated
        dup = {}
        for m in meta:
            if m["k"] == "el":
                key = (m["role"], m["name"])
                dup[key] = dup.get(key, 0) + 1
        lines, refs = [], []
        for i, m in enumerate(meta):
            if i not in keep:
                continue
            rid = ids[i]
            k = m["k"]
            name = m.get("name", "").replace("\n", " ")
            if k == "el":
                parts = [f'- {m["role"]} "{name}" [{rid}]']
                if m.get("value") not in (None, ""):
                    parts.append(f'value="{m["value"]}"')
                parts += m.get("flags", [])
                if m.get("href"):
                    parts.append(f'-> {m["href"][:120]}')
                if m.get("options") and m["role"] in ("combobox", "listbox"):
                    parts.append("options: " + " | ".join(o for o in m["options"] if o)[:200])
                if m.get("ctx") and dup.get((m["role"], m["name"]), 0) > 1:
                    parts.append(f'(in {m["ctx"]})')
                lines.append(" ".join(parts))
                refs.append({"ref": rid, "role": m["role"], "name": m["name"],
                             "value": m.get("value"), "enabled": "disabled" not in m.get("flags", [])})
            elif k == "heading":
                lines.append(f'- {m["role"]} "{name}" [{m.get("level")}]')
            elif k == "dialog":
                lines.append(f'- {m["role"]} "{name}" [{rid}]' + (" modal" if m.get("modal") else ""))
                refs.append({"ref": rid, "role": m["role"], "name": name, "value": None, "enabled": True})
            elif k == "table":
                lines.append(f'- table "{name}" [{rid}] {m["rows"]} rows; columns: ' + " | ".join(m["cols"]))
                for row in m.get("sample", []):
                    lines.append("    | " + " | ".join(row) + " |")
                if m["rows"] > len(m.get("sample", [])):
                    lines.append(f"    ... {m['rows'] - len(m.get('sample', []))} more rows (use extract)")
                refs.append({"ref": rid, "role": "table", "name": name, "value": None, "enabled": True})
            elif k == "list":
                lines.append(f'- list [{rid}] {m["count"]} items')
                refs.append({"ref": rid, "role": "list", "name": name, "value": None, "enabled": True})
            elif k == "alert":
                lines.append(f'- {m["role"]}: "{name}"')
            else:
                lines.append(f'- text: "{name}"')
        return "\n".join(lines), refs, complete, len(keep)

    async def op_inspect(self, req):
        tab = self.tab(req)
        kind = req.get("kind", "click")
        if tab.dialog is not None:
            raise self.dialog_error(tab, during=False)
        ref = await self.resolve(req, tab)
        if ref is None:
            info = await self.tab_info(tab)
            info.update(ref=None, facts=None)
            return info
        facts = await asyncio.wait_for(ref.handle.evaluate(FACTS, req.get("key")), 10)
        await self.preflight(tab, ref, kind, facts, req.get("preflight_ms", 3000))
        info = await self.tab_info(tab)
        info.update(ref=ref.id, obs=ref.obs, role=ref.fp["role"], name=ref.fp["name"], ctx=ref.fp["ctx"], facts=facts)
        return info

    async def preflight(self, tab, ref, kind, facts, ms):
        h = ref.handle
        try:
            if kind in ("click", "check", "download"):
                await self.guarded(tab, h.click(trial=True, timeout=ms), ms / 1000 + 5)
            elif kind == "fill":
                await h.wait_for_element_state("visible", timeout=ms)
                await h.wait_for_element_state("editable", timeout=ms)
            elif kind == "select":
                await h.wait_for_element_state("visible", timeout=ms)
                await h.wait_for_element_state("enabled", timeout=ms)
            elif kind == "upload":
                if not facts.get("file_input"):
                    await self.guarded(tab, h.click(trial=True, timeout=ms), ms / 1000 + 5)
            elif kind in ("press", "screenshot"):
                await h.wait_for_element_state("visible", timeout=ms)
        except OpError:
            raise
        except Exception as e:
            raise OpError("not_actionable", f"{ref.id} ({ref.fp['role']} \"{ref.fp['name']}\"): {pw_reason(e)}")

    async def op_act(self, req):
        tab = self.tab(req)
        kind = req["kind"]
        t = int(req.get("timeout_ms", 10000))
        ref = await self.resolve(req, tab) if (req.get("ref") or req.get("text")) else None
        nav_before, gen_before = tab.nav_count, tab.generation
        blocked_before = len(self.blocked)
        tabs_before = self.tab_counter
        modals_before = await self.modals(tab)
        try:
            result = await self.guarded(tab, self.do_act(tab, ref, kind, req, t), t / 1000 + 10)
        except OpError:
            raise
        except Exception as e:
            if is_actionability(e):
                raise OpError("not_actionable", pw_reason(e), effect="none")
            if "Timeout" in type(e).__name__:
                raise OpError("timeout", pw_reason(e), effect="unknown")
            raise OpError("action_failed", pw_reason(e), effect="unknown")
        navigated = await self.settle(tab, nav_before, gen_before, t)
        info = await self.tab_info(tab)
        info.update(result or {})
        info["navigated"] = navigated
        info["status"] = tab.last_status if navigated else None
        info["blocked"] = self.blocked[blocked_before:]
        info["new_tabs"] = [f"t{i}" for i in range(tabs_before + 1, self.tab_counter + 1)]
        if not navigated:
            opened = [m for m in await self.modals(tab) if m not in modals_before]
            if opened:
                info["modal"] = opened[0]
        return info

    async def modals(self, tab):
        """Names of the in-page modal dialogs showing now."""
        try:
            return await asyncio.wait_for(tab.page.evaluate(MODALS), 5)
        except Exception:
            return []

    async def do_act(self, tab, ref, kind, req, t):
        page = tab.page
        h = ref.handle if ref else None
        if kind == "click":
            await h.click(timeout=t)
            return {}
        if kind == "fill":
            value = req.get("value", "")
            await h.fill(value, timeout=t)
            actual = await h.evaluate("e => e.isContentEditable && !('value' in e) ? e.innerText : e.value")
            return {"value": actual}
        if kind == "select":
            want = req.get("value", "")
            options = await h.evaluate("e => Array.from(e.options).map(o => ({label: (o.label || o.text).replace(/\\s+/g,' ').trim(), value: o.value}))")
            match = [o for o in options if o["label"] == want] or [o for o in options if o["value"] == want] \
                or [o for o in options if o["label"].lower() == want.lower()]
            if not match:
                raise OpError("not_found", f"no option {want!r}; options are: " + ", ".join(o["label"] for o in options)[:300])
            await h.select_option(value=match[0]["value"], timeout=t)
            return {"value": match[0]["label"]}
        if kind == "check":
            await h.set_checked(bool(req.get("checked", True)), timeout=t)
            return {"checked": await h.evaluate("e => !!e.checked")}
        if kind == "press":
            key = req["key"]
            if h is not None:
                await h.press(key, timeout=t)
            else:
                await page.keyboard.press(key)
            return {}
        if kind == "upload":
            path = req["path"]
            is_file = await h.evaluate("e => e.tagName === 'INPUT' && e.type === 'file'")
            if is_file:
                await h.set_input_files(path, timeout=t)
            else:
                async with page.expect_file_chooser(timeout=t) as chooser:
                    await h.click(timeout=t)
                await (await chooser.value).set_files(path)
            return {"files": [os.path.basename(path)]}
        if kind == "download":
            seen = len(tab.downloads)
            await h.click(timeout=t)
            download = await self.next_download(tab, seen, t)
            return {"download": await self.save_download(download, req, t)}
        raise OpError("invalid_input", f"unknown action {kind}")

    async def next_download(self, tab, index, t):
        deadline = time.monotonic() + t / 1000
        while len(tab.downloads) <= index:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise OpError("timeout", f"no download started within {t} ms", effect="unknown")
            tab.download_event.clear()
            if len(tab.downloads) > index:
                break
            try:
                await asyncio.wait_for(tab.download_event.wait(), remaining)
            except asyncio.TimeoutError:
                pass
        tab.consumed = max(tab.consumed, index + 1)
        return tab.downloads[index]

    async def save_download(self, download, req, t):
        try:
            await asyncio.wait_for(download.path(), t / 1000)
        except asyncio.TimeoutError:
            raise OpError("timeout", f"the download of {download.suggested_filename} did not finish within {t} ms",
                          effect="unknown")
        except Exception as e:
            raise OpError("download_failed", pw_reason(e), effect="done")
        failure = await download.failure()
        if failure:
            raise OpError("download_failed", f"download failed: {failure}", effect="done")
        dest = req["dest"]
        if req.get("dest_is_dir"):
            name = os.path.basename(download.suggested_filename) or "download"
            dest = os.path.join(dest, name)
        await download.save_as(dest)
        with open(dest, "rb") as f:
            data = f.read()
        return {"path": dest, "suggested": download.suggested_filename, "url": download.url,
                "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}

    async def op_wait(self, req):
        tab = self.tab(req)
        t = int(req.get("timeout_ms", 10000))
        page = tab.page
        if req.get("download"):
            download = await self.next_download(tab, tab.consumed, t)
            info = await self.tab_info(tab)
            info["download"] = await self.save_download(download, req, t)
            return info
        if req.get("text") is not None:
            coro = page.wait_for_function("t => document.body && document.body.innerText.includes(t)",
                                          arg=req["text"], timeout=t)
        elif req.get("text_gone") is not None:
            coro = page.wait_for_function("t => !document.body || !document.body.innerText.includes(t)",
                                          arg=req["text_gone"], timeout=t)
        elif req.get("url_contains") is not None:
            part = req["url_contains"]
            coro = page.wait_for_url(lambda u: part in u, timeout=t, wait_until="load")
        elif req.get("selector"):
            coro = page.wait_for_selector(req["selector"], state=req.get("state") or "visible", timeout=t)
        elif req.get("load"):
            coro = page.wait_for_load_state(req["load"], timeout=t)
        else:
            raise OpError("invalid_input", "wait needs one of text, text_gone, url_contains, selector, load or download")
        gen_before = tab.generation
        try:
            await self.guarded(tab, coro, t / 1000 + 5)
        except OpError:
            raise
        except Exception as e:
            if "Timeout" in type(e).__name__:
                raise OpError("timeout", f"the condition did not hold within {t} ms")
            raise OpError("wait_failed", pw_reason(e))
        info = await self.tab_info(tab)
        info["navigated"] = tab.generation != gen_before
        return info

    async def op_check(self, req):
        tab = self.tab(req)
        page = tab.page
        problems = []
        body = await page.evaluate("() => document.body ? document.body.innerText : ''")
        if req.get("text") is not None and req["text"] not in body:
            problems.append(f"text {req['text']!r} is not on the page")
        if req.get("text_gone") is not None and req["text_gone"] in body:
            problems.append(f"text {req['text_gone']!r} is still on the page")
        if req.get("url_contains") is not None and req["url_contains"] not in page.url:
            problems.append(f"url {page.url} does not contain {req['url_contains']!r}")
        if req.get("ref"):
            ref = await self.resolve({"ref": req["ref"]}, tab)
            actual = await ref.handle.evaluate(
                "e => e.tagName === 'SELECT' ? Array.from(e.selectedOptions).map(o => o.label || o.text).join(', ') : ('value' in e ? e.value : e.innerText)")
            if req.get("value") is not None and actual != req["value"] and \
                    not (await ref.handle.evaluate("e => e.tagName === 'SELECT'") and
                         await ref.handle.evaluate("(e, v) => e.value === v", req["value"])):
                problems.append(f"{req['ref']} has value {actual!r}, expected {req['value']!r}")
        info = await self.tab_info(tab)
        info["passed"] = not problems
        info["problems"] = problems
        return info

    async def op_extract(self, req):
        tab = self.tab(req)
        t = int(req.get("timeout_ms", 10000))
        max_pages = max(1, min(int(req.get("max_pages", 1)), 20))
        if req.get("ref"):
            root = (await self.resolve({"ref": req["ref"]}, tab)).handle
        elif req.get("selector"):
            root = (await tab.page.evaluate_handle("s => document.querySelector(s)", req["selector"])).as_element()
            if root is None:
                raise OpError("not_found", f"nothing matches {req['selector']!r}")
        else:
            root = (await tab.page.evaluate_handle("() => document.querySelector('table') || document.querySelector('ul,ol')")).as_element()
            if root is None:
                raise OpError("not_found", "the page has no table or list; give a ref or selector")
        pages = []
        cols = None
        kind = None
        note = None
        complete = True
        seen = set()
        for page_no in range(max_pages):
            data = await self.guarded(tab, root.evaluate(EXTRACT), t / 1000)
            if data is None:
                raise OpError("not_found", "the target is not a table or list")
            kind = data["kind"]
            if cols is None:
                cols = data["cols"]
            fingerprint = tab.page.url + "\n" + data["text"]
            if fingerprint in seen:
                note = "the next page showed the same content; stopped"
                break
            seen.add(fingerprint)
            pages.append(data["rows"])
            nxt = req.get("next")
            if not nxt:
                break
            if page_no == max_pages - 1:
                complete = False
                note = f"stopped after max_pages={max_pages}; more pages may follow"
                break
            meta, handles, _ = await self.collect(tab, FIND, {"text": nxt})
            if not meta:
                break
            if len(meta) > 1:
                complete = False
                note = f"{len(meta)} controls are named {nxt!r}; stopped"
                break
            target = handles[0]
            disabled = await target.evaluate("e => e.disabled || e.getAttribute('aria-disabled') === 'true'")
            if disabled:
                break
            nav_before, gen_before = tab.nav_count, tab.generation
            before_text = data["text"]
            await self.guarded(tab, target.click(timeout=t), t / 1000 + 5)
            navigated = await self.settle(tab, nav_before, gen_before, t)
            if navigated:
                found = await tab.page.evaluate_handle(FIND_TABLE, cols) if kind == "table" else None
                el = found.as_element() if found is not None else None
                if el is None:
                    complete = False
                    note = "could not find the same table on the next page; stopped"
                    break
                root = el
            else:
                try:
                    await tab.page.wait_for_function("([r, t]) => r.innerText.replace(/\\s+/g,' ').trim().slice(0, 4000) !== t",
                                                     arg=[root, before_text], timeout=t)
                except Exception:
                    complete = False
                    note = "the next control did not change the content; stopped"
                    break
        rows = []
        names = []
        for i, c in enumerate(cols or []):
            n = c or f"col{i + 1}"
            while n in names:
                n += "_"
            names.append(n)
        for page_rows in pages:
            for r in page_rows:
                keys = names if len(names) >= len(r) else names + [f"col{i + 1}" for i in range(len(names), len(r))]
                rows.append({keys[i]: v for i, v in enumerate(r)})
        info = await self.tab_info(tab)
        info.update(kind=kind, columns=names, rows=rows, pages=len(pages), complete=complete, note=note)
        return info

    async def op_screenshot(self, req):
        tab = self.tab(req)
        page = tab.page
        dpr = await page.evaluate("() => window.devicePixelRatio")
        if req.get("ref"):
            ref = await self.resolve({"ref": req["ref"]}, tab)
            await self.preflight(tab, ref, "screenshot", {}, 3000)
            await ref.handle.scroll_into_view_if_needed(timeout=5000)
            box = await ref.handle.bounding_box()
            data = await ref.handle.screenshot(type="png", timeout=10000)
            crop = [box["x"], box["y"], box["width"], box["height"]] if box else [0, 0, 0, 0]
        else:
            size = await page.evaluate("() => [window.innerWidth, window.innerHeight]")
            data = await page.screenshot(type="png", timeout=10000)
            crop = [0, 0, size[0], size[1]]
        info = await self.tab_info(tab)
        info.update(png=base64.b64encode(data).decode(), crop=crop, scale=dpr, ts=int(time.time() * 1000))
        return info

    async def op_tabs(self, req):
        if req.get("select"):
            tab = self.tab({"tab": req["select"]})
            await tab.page.bring_to_front()
        out = []
        for tab in list(self.tabs.values()):
            if tab.closed:
                continue
            out.append(await self.tab_info(tab))
        return {"tabs": out}

    async def op_close(self, req):
        tab = self.tab(req)
        try:
            await tab.page.close()
        except Exception:
            pass
        tab.closed = True
        self.tabs.pop(tab.id, None)
        for rid in [r for r, v in self.refs.items() if v.tab == tab.id]:
            del self.refs[rid]
        return {"tab": tab.id, "closed": True}

    async def op_dialog(self, req):
        tab = self.tabs.get(req.get("tab"))
        if tab is None or tab.closed:
            raise OpError("not_found", f"There is no open tab {req.get('tab')}.")
        d = tab.dialog
        if d is None:
            raise OpError("not_found", "no native dialog is open in this tab")
        info = {"type": d.type, "message": d.message[:500]}
        if req.get("peek"):
            # No page calls here: they would wait behind the open dialog.
            return {"tab": tab.id, "url": tab.page.url, "origin": origin_of(tab.page.url), "dialog": info}
        nav_before, gen_before = tab.nav_count, tab.generation
        if req.get("accept"):
            if req.get("prompt_text") is not None:
                await d.accept(req["prompt_text"])
            else:
                await d.accept()
        else:
            await d.dismiss()
        tab.dialog = None
        tab.dialog_event.clear()
        stalled, tab.stalled = tab.stalled, None
        t = int(req.get("timeout_ms", 10000))
        if stalled is not None:
            waiter = asyncio.ensure_future(tab.dialog_event.wait())
            done, _ = await asyncio.wait({stalled, waiter}, timeout=t / 1000, return_when=asyncio.FIRST_COMPLETED)
            if not waiter.done():
                waiter.cancel()
            if stalled not in done and tab.dialog is not None:
                tab.stalled = stalled
        navigated = False
        if tab.dialog is None:
            navigated = await self.settle(tab, nav_before, gen_before, t)
        out = await self.tab_info(tab)
        out.update(dialog=info, accepted=bool(req.get("accept")), navigated=navigated,
                   another_dialog=({"type": tab.dialog.type, "message": tab.dialog.message[:500]}
                                   if tab.dialog is not None else None))
        return out

    async def op_shutdown(self, req):
        await self.close()
        return {}

    async def close(self):
        for ctx in list(self.contexts.values()):
            try:
                await asyncio.wait_for(ctx.close(), 10)
            except Exception:
                pass
        self.contexts.clear()
        if self.pw is not None:
            try:
                await asyncio.wait_for(self.pw.stop(), 10)
            except Exception:
                pass
            self.pw = None
        shutil.rmtree(self.download_dir, ignore_errors=True)

    async def handle(self, req):
        op = req.get("op")
        fn = getattr(self, f"op_{op}", None)
        if fn is None:
            raise OpError("invalid_input", f"unknown operation {op!r}")
        if op != "hello" and self.pw is None:
            raise OpError("protocol", "hello must come first")
        return await fn(req)

    def take_events(self):
        events, self.events = self.events, []
        return events


async def main():
    helper = Helper()
    loop = asyncio.get_running_loop()
    stdin = sys.stdin.buffer
    while True:
        line = await loop.run_in_executor(None, stdin.readline)
        if not line:
            break
        try:
            req = json.loads(line.decode("utf-8"))
        except ValueError:
            send({"id": None, "ok": False, "error": {"code": "protocol", "message": "unreadable request", "effect": "none"}})
            continue
        rid = req.get("id")
        try:
            res = await helper.handle(req) or {}
            res.update(id=rid, ok=True)
        except OpError as e:
            res = {"id": rid, "ok": False, "error": dict(code=e.code, message=e.message, effect=e.effect, **e.extra)}
        except ImportError as e:
            res = {"id": rid, "ok": False, "error": {"code": "helper_unavailable",
                                                     "message": f"Playwright for Python is not installed: {e}",
                                                     "effect": "none"}}
        except Exception as e:
            name = type(e).__name__
            code = "timeout" if "Timeout" in name else "internal"
            res = {"id": rid, "ok": False, "error": {"code": code, "message": f"{name}: {pw_reason(e)}",
                                                     "effect": "unknown"}}
        events = helper.take_events()
        if events:
            res["events"] = events
        res["counters"] = [helper.ref_counter, helper.tab_counter, helper.obs_counter]
        send(res)
        if req.get("op") == "shutdown":
            break
    await helper.close()
    _OUT.flush()
    os._exit(0)


if __name__ == "__main__":
    asyncio.run(main())
