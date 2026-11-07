// Prints the Sankey output of the design script, the expected values of tests/layout.rs.
// Regenerate after a design change:
//   node crates/aivory-protocol/tests/design-sankey.js docs/design/screens/Main.dc.html main > crates/aivory-protocol/tests/sankey-main.json
//   node crates/aivory-protocol/tests/design-sankey.js docs/design/screens/Repos.dc.html repos > crates/aivory-protocol/tests/sankey-repos.json
const fs = require('fs');
const [file, mode] = process.argv.slice(2);
const html = fs.readFileSync(file, 'utf8');
const m = html.match(/<script type="text\/x-dc" data-dc-script[^>]*>([\s\S]*?)<\/script>/);
const src = 'class DCLogic { constructor(p) { this.props = p || {}; this.state = {}; } setState() {} }\n' + m[1] +
  '\nmodule.exports = { Component, CB, S };';
const mod = { exports: {} };
new Function('module', 'window', src)(mod, undefined);
const { Component, CB, S } = mod.exports;
const c = new Component({});
const out = {};
for (const [id] of CB) {
  const live = mode === 'repos' ? S.filter(s => s[1] === id) : S.filter(s => 'qewi'.includes(s.st) && s.cb === id);
  if (!live.length) continue;
  const r = c.sankey(id, live);
  out[id] = { links: r.links.map(l => ({ d: l.d, w: l.w })), nodes: r.nodes.map(n => [n.x, n.y, n.h]), labels: r.labels.map(l => [l.y, l.t]) };
}
console.log(JSON.stringify(out));
