const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function app() {
  const context = { fillRect() {}, createImageData(w, h) { return { data: new Uint8ClampedArray(w*h*4) }; }, putImageData() {} };
  const element = () => ({ getContext: () => context, addEventListener() {}, setAttribute() {}, classList: { toggle() {} } });
  const elements = new Map();
  const sandbox = vm.createContext({
    document: { querySelectorAll() { return []; }, querySelector(id) { if (!elements.has(id)) elements.set(id, element()); return elements.get(id); } },
    window: { addEventListener() {} },
    Uint8Array, Uint8ClampedArray,
  });
  const source = `const GLYPHS = ${fs.readFileSync(`${__dirname}/../assets/font.json`, 'utf8')};\n` + fs.readFileSync(`${__dirname}/../assets/app.js`, 'utf8');
  // Skip network startup; exercise the actual drawing handlers and serialization.
  vm.runInContext(source.replace(/refreshStatus\(\);\nconnectRoomSocket\(\);\s*$/, ''), sandbox);
  return code => vm.runInContext(code, sandbox);
}

test('rainbow stroke survives packing, with paper erased and text remaining monochrome', () => {
  const run = app();
  run('selectInk("rainbow"); state.lastPoint = {x:30,y:30}; drawTo({x:230,y:30});');
  const pixels = run('unpackBitmap(packedDrawing())');
  assert.deepEqual(new Set(pixels), new Set([0, ...Array.from({length:13}, (_,i)=>i+3)]));
  assert.deepEqual(pixels, run('state.inkPixels'));
  run('selectTool("eraser"); state.lastPoint = {x:100,y:30}; drawTo({x:100,y:30});');
  assert.equal(run('unpackBitmap(packedDrawing())[30*256+100]'), 0);
  run('selectInk(1); renderText("HELLO")');
  assert.deepEqual(new Set(run('unpackBitmap(packedDrawing())')), new Set([0,1]));
  run('clearDrawing()');
  assert.ok(run('packedDrawing().every(byte => byte === 0)'));
});

test('known bytes decode low-nibble-first and legacy ink remains black', () => {
  const run = app();
  assert.deepEqual(Array.from(run('unpackBitmap(Uint8Array.from({length:12288}, (_,i)=>i===0?0xf3:0)).slice(0,4)')), [3,15,0,0]);
  assert.deepEqual(Array.from(run('unpackBitmap(Uint8Array.from({length:3072}, (_,i)=>i===0?0x81:0)).slice(0,8)')), [1,0,0,0,0,0,0,1]);
  assert.throws(() => run('unpackBitmap(new Uint8Array(1))'), /Invalid bitmap length/);
});


test('brush clips to the DS inset and name-tag cutout, including its radius', () => {
  const run = app();
  run('state.lastPoint = {x:0,y:8}; drawTo({x:255,y:8}); state.lastPoint = {x:0,y:30}; drawTo({x:255,y:30}); state.lastPoint = {x:100,y:0}; drawTo({x:100,y:95});');
  const pixels = run('unpackBitmap(packedDrawing())');
  assert.equal(pixels[8*256+79], 0);
  assert.equal(pixels[8*256+80], 1);
  assert.equal(pixels[30*256+23], 0);
  assert.equal(pixels[30*256+24], 1);
  assert.equal(pixels[30*256+251], 1);
  assert.equal(pixels[30*256+252], 0);
  assert.equal(pixels[79*256+100], 1);
  assert.equal(pixels[80*256+100], 0);
  assert.ok(run('state.inkPixels.every((ink, i) => ink === 0 || isDrawablePixel(i % WIDTH, Math.floor(i / WIDTH)))'));
});


test('text wraps whole words and discards the space at the wrap', () => {
  const run = app();
  assert.deepEqual(run('renderTextWithLayout("hello beautiful world", LARGE_TEXT)'),
    run('renderTextWithLayout("hello\\nbeautiful world", LARGE_TEXT)'));
  assert.deepEqual(run('renderTextWithLayout("hello   beautiful", LARGE_TEXT)'),
    run('renderTextWithLayout("hello\\nbeautiful", LARGE_TEXT)'));
});

test('text keeps explicit newlines, splits oversized words, and detects overflow', () => {
  const run = app();
  assert.deepEqual(run('renderTextWithLayout("ABCDEFGHIJKLMN", LARGE_TEXT)'),
    run('renderTextWithLayout("ABCDEFGHIJKLM\\nN", LARGE_TEXT)'));
  assert.deepEqual(run('renderTextWithLayout("a\\r\\nb", LARGE_TEXT)'),
    run('renderTextWithLayout("a\\nb", LARGE_TEXT)'));
  assert.equal(run('renderTextWithLayout("word ".repeat(100), LARGE_TEXT)'), null);
});


test('fixed ink persists through tool changes and clearing, and is preserved on send', () => {
  const run = app();
  for (const ink of [1,3,4,5,6,7,8,9,10,11,12,13,14,15]) {
    run(`selectInk("rainbow"); selectInk(${ink}); clearDrawing(); state.lastPoint = {x:100,y:30}; drawTo({x:120,y:30});`);
    assert.equal(run('state.tool'), 'pen');
    assert.deepEqual(new Set(run('unpackBitmap(packedDrawing())')), new Set([0,ink]));
    run('selectTool("eraser"); selectTool("pen")');
    assert.equal(run('state.penInk'), ink);
  }
  run('selectInk(2)');
  assert.equal(run('state.penInk'), 15);
});


test('text uses selected ink without changing glyphs, including rainbow after erasing', () => {
  const run = app();
  run('selectInk(1); renderText("HELLO WORLD")');
  const shape = Array.from(run('state.inkPixels'), ink => ink !== 0);
  for (const ink of [5, 11, 15, 'rainbow']) {
    run(`selectInk(${JSON.stringify(ink)}); selectTool("eraser"); renderText("HELLO WORLD")`);
    const pixels = run('unpackBitmap(packedDrawing())');
    assert.deepEqual(Array.from(pixels, ink => ink !== 0), shape);
    const colors = new Set(pixels.filter(ink => ink !== 0));
    if (ink === 'rainbow') {
      assert.ok(colors.size > 5);
      assert.ok([...colors].every(color => color >= 3 && color <= 15));
    } else assert.deepEqual(colors, new Set([ink]));
  }
  run('selectTool("pen"); clearDrawing(); state.lastPoint = {x:30,y:30}; drawTo({x:230,y:30});');
  assert.ok(new Set(run('state.inkPixels')).size > 5);
});

test('browser text rendering matches the REST golden samples', () => {
  const run = app();
  const fixtures = JSON.parse(fs.readFileSync(`${__dirname}/text-fixtures.json`, 'utf8'));
  for (const {text, ink, hash} of fixtures) {
    run(`selectInk(${JSON.stringify(ink)}); renderText(${JSON.stringify(text)})`);
    const actual = run('packedDrawing()').reduce((h,b) => Math.imul(h^b,16777619)>>>0,2166136261);
    assert.equal(actual,hash,text);
  }
});
