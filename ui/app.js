const { invoke } = window.__TAURI__.core;
const { open, save, ask } = window.__TAURI__.dialog;

const $ = (id) => document.getElementById(id);
const pdfFilter = [{ name: 'PDF', extensions: ['pdf'] }];
// Sizes in points. A click without a drag places a field of the default size.
const DEFAULT_SIZE = [216, 40];
const MIN_SIZE = [36, 14];

const state = {
  path: null,
  // From inspect_pdf: pages, fields, taken, can_add, can_sign, notice.
  info: null,
  // Fields drawn on the pages and not saved yet: { id, name, page, rect: [left, top, width, height] }.
  added: [],
  // Key of the field to sign (see keyOf).
  selected: null,
  identities: [],
  lastOutput: null,
  busy: false,
  // Bumped on every load, so page images still rendering for an older file are dropped.
  load: 0,
  urls: [],
};
let nextId = 1;
let drag = null;

// PDF field names and signer names are untrusted, so text only goes in through append/textContent.
function el(tag, props = {}, ...children) {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
}

const clamp = (v, lo, hi) => Math.min(Math.max(v, lo), Math.max(lo, hi));
const keyOf = (f) => (f.id ? `new:${f.id}` : `existing:${f.name}`);
const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;

function setStatus(text, kind = '') {
  $('status').textContent = text;
  $('status').className = kind;
  $('reveal').hidden = kind !== 'ok';
}

function selectedField() {
  if (!state.info) return null;
  return [...state.added, ...state.info.fields].find((f) => keyOf(f) === state.selected) ?? null;
}

function nameProblem(field) {
  const name = field.name.trim();
  if (!name) return 'Give the field a name.';
  if (name.includes('.')) return 'Field names cannot contain periods.';
  const used = state.info.taken.includes(name) || state.added.some((f) => f !== field && f.name.trim() === name);
  return used ? 'Another field has this name.' : '';
}

function uniqueName() {
  const used = new Set([...state.info.taken, ...state.added.map((f) => f.name.trim())]);
  for (let i = 1; ; i++) {
    if (!used.has(`Signature${i}`)) return `Signature${i}`;
  }
}

function updateButtons() {
  if (!state.info) {
    $('sign').disabled = true;
    $('save-fields').hidden = true;
    return;
  }
  const field = selectedField();
  const namesOk = state.added.every((f) => !nameProblem(f));
  const signable = field && !field.signature && state.info.can_sign;
  $('sign').disabled = state.busy || !signable || !namesOk || !$('identity').value;
  $('save-fields').hidden = !state.added.length;
  $('save-fields').disabled = state.busy || !namesOk;
}

// Certificates

function describeIdentity() {
  const id = state.identities.find((i) => i.id === $('identity').value);
  $('identity-note').textContent = id
    ? `Issued by ${id.issuer} · expires ${id.expires}`
    : state.identities.length
      ? 'Every certificate on the card has expired.'
      : 'No CAC found. Insert your card and press Refresh.';
}

async function loadIdentities() {
  $('identity').replaceChildren();
  $('identity-note').textContent = 'Reading card…';
  try {
    state.identities = await invoke('list_identities');
  } catch (e) {
    state.identities = [];
    $('identity-note').textContent = String(e);
    updateButtons();
    return;
  }
  for (const id of state.identities) {
    const label = `${id.kind} · ${id.name}${id.expired ? ' (expired)' : ''}`;
    $('identity').append(el('option', { value: id.id, disabled: id.expired }, label));
  }
  $('identity').value = state.identities.find((i) => !i.expired)?.id ?? '';
  describeIdentity();
  updateButtons();
}

// Field list

function signatureState(sig) {
  const when = sig.time ? ` · ${sig.time}` : '';
  if (sig.status === 'valid') return [`✓ Signed by ${sig.signer}${when}`, 'ok'];
  if (sig.status === 'untrusted') {
    return [`Signed by ${sig.signer}${when} · certificate not trusted on this Mac`, 'warn'];
  }
  return [`✗ ${sig.detail || 'The signature does not verify.'}`, 'error'];
}

function radioFor(f) {
  const radio = el('input', {
    type: 'radio',
    name: 'field',
    checked: keyOf(f) === state.selected,
    disabled: Boolean(f.signature) || !state.info.can_sign,
  });
  radio.addEventListener('change', () => select(keyOf(f), true));
  return radio;
}

function existingRow(f) {
  const [text, kind] = f.signature ? signatureState(f.signature) : ['Not signed', 'muted'];
  return el(
    'li',
    {},
    el(
      'label',
      { className: 'item' },
      radioFor(f),
      el('span', { className: 'name' }, f.name),
      el('span', { className: 'where' }, f.page ? `Page ${f.page}` : ''),
      el('span', { className: `state ${kind}`, title: f.signature?.detail ?? '' }, text),
    ),
  );
}

function newRow(f) {
  const name = el('input', { type: 'text', value: f.name, spellcheck: false, title: 'Field name' });
  const note = el('span');
  // Renaming one field can clear or cause a clash with another, so every row refreshes.
  f.refresh = () => {
    const problem = nameProblem(f);
    note.textContent = problem || 'New field. It is saved with the PDF.';
    note.className = `state ${problem ? 'error' : 'muted'}`;
    name.classList.toggle('invalid', Boolean(problem));
  };
  name.addEventListener('input', () => {
    f.name = name.value;
    for (const other of state.added) other.refresh();
    const tag = $('pages').querySelector(`.box[data-key="${keyOf(f)}"] .tag`);
    if (tag) tag.textContent = f.name || 'Unnamed';
    updateButtons();
  });
  const remove = el('button', { type: 'button', className: 'remove', title: 'Remove this field' }, '×');
  remove.addEventListener('click', () => removeField(f));
  f.refresh();
  return el(
    'li',
    {},
    el('div', { className: 'item new' }, radioFor(f), name, el('span', { className: 'where' }, `Page ${f.page}`), remove, note),
  );
}

function renderFields() {
  const { fields, can_add, can_sign, notice } = state.info;
  const signable = [...state.added, ...fields.filter((f) => !f.signature)];
  if (!can_sign || !signable.some((f) => keyOf(f) === state.selected)) {
    state.selected = can_sign && signable[0] ? keyOf(signable[0]) : null;
  }
  const rows = [...fields.map(existingRow), ...state.added.map(newRow)];
  $('fields').replaceChildren(
    ...(rows.length ? rows : [el('li', { className: 'empty' }, 'This PDF has no signature fields yet.')]),
  );
  $('fields-note').textContent = notice ?? (can_add ? 'Click or drag on a page to add a signature field.' : '');
  renderBoxes();
  updateButtons();
}

function select(key, scroll) {
  state.selected = key;
  renderFields();
  if (scroll) {
    $('pages').querySelector(`.box[data-key="${CSS.escape(key)}"]`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }
}

function removeField(f) {
  state.added = state.added.filter((other) => other !== f);
  renderFields();
}

// Pages and the boxes drawn over them

const renderQueue = [];
let rendering = false;
const observer = new IntersectionObserver(
  (entries) => {
    for (const entry of entries) {
      if (entry.isIntersecting && !entry.target.dataset.queued) {
        entry.target.dataset.queued = '1';
        renderQueue.push(entry.target);
      }
    }
    drawPages();
  },
  { root: $('viewer'), rootMargin: '800px 0px' },
);

async function drawPages() {
  if (rendering) return;
  rendering = true;
  while (renderQueue.length) {
    const page = renderQueue.shift();
    const { load, path } = state;
    if (!page.isConnected) continue;
    const width = Math.min(3000, Math.ceil((page.clientWidth * devicePixelRatio) / 100) * 100);
    try {
      const png = await invoke('render_page', { path, page: Number(page.dataset.page), width });
      if (load !== state.load) continue;
      const url = URL.createObjectURL(new Blob([png], { type: 'image/png' }));
      state.urls.push(url);
      page.querySelector('img').src = url;
    } catch {
      // The page stays blank; the boxes over it still work.
    }
  }
  rendering = false;
}

function buildPages() {
  for (const url of state.urls) URL.revokeObjectURL(url);
  state.urls = [];
  observer.disconnect();
  renderQueue.length = 0;
  const total = state.info.pages.length;
  const nodes = state.info.pages.flatMap(([w, h], i) => {
    const page = el('div', { className: `page${state.info.can_add ? '' : ' locked'}` }, el('img', { alt: '', draggable: false }));
    page.dataset.page = String(i + 1);
    page.style.aspectRatio = `${w} / ${h}`;
    page.addEventListener('pointerdown', onPointerDown);
    observer.observe(page);
    return [page, el('p', { className: 'page-number' }, `Page ${i + 1} of ${total}`)];
  });
  $('pages').replaceChildren(...nodes);
}

function place(node, f) {
  const [w, h] = state.info.pages[f.page - 1];
  const [left, top, width, height] = f.rect;
  Object.assign(node.style, {
    left: `${(left / w) * 100}%`,
    top: `${(top / h) * 100}%`,
    width: `${(width / w) * 100}%`,
    height: `${(height / h) * 100}%`,
  });
}

function box(f, kind) {
  const selected = keyOf(f) === state.selected;
  const node = el('div', { className: `box ${kind}${selected ? ' selected' : ''}`, title: f.name });
  node.dataset.key = keyOf(f);
  if (kind === 'new') {
    node.append(el('span', { className: 'tag' }, f.name || 'Unnamed'));
    if (selected) node.append(el('span', { className: 'handle' }));
  }
  place(node, f);
  return node;
}

function renderBoxes() {
  const pages = [...$('pages').querySelectorAll('.page')];
  for (const page of pages) page.querySelectorAll('.box').forEach((b) => b.remove());
  for (const f of state.info.fields) {
    if (f.page && f.rect) pages[f.page - 1]?.append(box(f, f.signature ? 'signed' : 'existing'));
  }
  for (const f of state.added) pages[f.page - 1]?.append(box(f, 'new'));
}

// Placing fields: drag on a page to draw one, click for one of the default size, drag a new
// field to move it, or drag its corner handle to resize it.

function pointIn(page, event) {
  const r = page.getBoundingClientRect();
  const [w, h] = state.info.pages[page.dataset.page - 1];
  return [clamp(((event.clientX - r.left) / r.width) * w, 0, w), clamp(((event.clientY - r.top) / r.height) * h, 0, h)];
}

function onPointerDown(event) {
  if (event.button !== 0 || state.busy || drag) return;
  const page = event.currentTarget;
  const at = pointIn(page, event);
  const key = event.target.closest('.box')?.dataset.key;
  const added = state.added.find((f) => keyOf(f) === key);
  if (added) {
    const mode = event.target.classList.contains('handle') ? 'resize' : 'move';
    if (state.selected !== key) select(key, false);
    drag = { page, field: added, mode, start: at, rect: [...added.rect] };
  } else if (key) {
    const field = state.info.fields.find((f) => keyOf(f) === key);
    if (field && !field.signature && state.info.can_sign) select(key, false);
    return;
  } else if (state.info.can_add) {
    const field = { id: nextId++, name: uniqueName(), page: Number(page.dataset.page), rect: [...at, 0, 0] };
    drag = { page, field, mode: 'draw', start: at, node: box(field, 'new') };
    page.append(drag.node);
  } else {
    return;
  }
  event.preventDefault();
  page.setPointerCapture(event.pointerId);
  page.addEventListener('pointermove', onPointerMove);
  page.addEventListener('pointerup', onPointerUp);
  page.addEventListener('pointercancel', onPointerUp);
}

function onPointerMove(event) {
  const { page, field, mode, start } = drag;
  const [x, y] = pointIn(page, event);
  const [w, h] = state.info.pages[field.page - 1];
  if (mode === 'draw') {
    field.rect = [Math.min(start[0], x), Math.min(start[1], y), Math.abs(x - start[0]), Math.abs(y - start[1])];
  } else if (mode === 'move') {
    const [left, top, width, height] = drag.rect;
    field.rect = [clamp(left + x - start[0], 0, w - width), clamp(top + y - start[1], 0, h - height), width, height];
  } else {
    const [left, top] = drag.rect;
    field.rect = [left, top, clamp(x - left, MIN_SIZE[0], w - left), clamp(y - top, MIN_SIZE[1], h - top)];
  }
  const node = drag.node ?? page.querySelector(`.box[data-key="${keyOf(field)}"]`);
  if (node) place(node, field);
}

function onPointerUp() {
  const { page, field, mode, cancelled } = drag;
  page.removeEventListener('pointermove', onPointerMove);
  page.removeEventListener('pointerup', onPointerUp);
  page.removeEventListener('pointercancel', onPointerUp);
  drag = null;
  if (mode === 'draw' && !cancelled) {
    const [w, h] = state.info.pages[field.page - 1];
    const [left, top, width, height] = field.rect;
    if (width < 6 && height < 6) {
      const [dw, dh] = [Math.min(DEFAULT_SIZE[0], w), Math.min(DEFAULT_SIZE[1], h)];
      field.rect = [clamp(left - dw / 2, 0, w - dw), clamp(top - dh / 2, 0, h - dh), dw, dh];
    } else {
      const [fw, fh] = [Math.max(width, MIN_SIZE[0]), Math.max(height, MIN_SIZE[1])];
      field.rect = [clamp(left, 0, w - fw), clamp(top, 0, h - fh), fw, fh];
    }
    state.added.push(field);
    state.selected = keyOf(field);
    setStatus('');
  }
  renderFields();
}

document.addEventListener('keydown', (event) => {
  if (event.target.closest('input, select, textarea')) return;
  if (event.key === 'Escape' && drag?.mode === 'draw') {
    drag.cancelled = true;
    drag.node.remove();
  } else if ((event.key === 'Delete' || event.key === 'Backspace') && !drag) {
    const field = state.added.find((f) => keyOf(f) === state.selected);
    if (field) {
      event.preventDefault();
      removeField(field);
    }
  }
});

// Opening, signing, saving

// After signing or saving, the same document comes back, so the view stays where it was.
async function loadPdf(path, keepView = false) {
  const load = ++state.load;
  let info;
  try {
    info = await invoke('inspect_pdf', { path });
  } catch (e) {
    if (load !== state.load) return;
    Object.assign(state, { path: null, info: null, added: [], selected: null });
    $('welcome').hidden = false;
    $('pages').hidden = true;
    $('doc-section').hidden = true;
    $('fields-section').hidden = true;
    $('pages').replaceChildren();
    updateButtons();
    setStatus(String(e), 'error');
    return;
  }
  if (load !== state.load) return;
  Object.assign(state, { path, info, added: [], selected: null });
  $('file-name').textContent = path.split('/').pop();
  $('file-dir').textContent = path.slice(0, path.lastIndexOf('/'));
  $('welcome').hidden = true;
  $('pages').hidden = false;
  $('doc-section').hidden = false;
  $('fields-section').hidden = false;
  if (!keepView) $('viewer').scrollTop = 0;
  buildPages();
  renderFields();
}

async function openPdf(path) {
  if (state.added.length) {
    const discard = await ask(`Discard the ${plural(state.added.length, 'field')} you added and not saved?`, {
      title: 'Open another PDF',
      kind: 'warning',
      okLabel: 'Discard',
      cancelLabel: 'Cancel',
    });
    if (!discard) return;
  }
  setStatus('');
  await loadPdf(path);
}

async function choosePdf() {
  const path = await open({ multiple: false, directory: false, filters: pdfFilter });
  if (path) await openPdf(path);
}

function newFields() {
  return state.added.map((f) => ({ name: f.name.trim(), page: f.page, rect: f.rect }));
}

async function run(busyText, work) {
  state.busy = true;
  updateButtons();
  setStatus(busyText, 'busy');
  try {
    setStatus(await work(), 'ok');
  } catch (e) {
    setStatus(String(e), 'error');
  } finally {
    state.busy = false;
    updateButtons();
  }
}

async function sign() {
  const field = selectedField();
  if (!field) return;
  const name = field.name.trim();
  const output = await save({
    defaultPath: state.path.replace(/(_signed)?\.pdf$/i, '_signed.pdf'),
    filters: pdfFilter,
  });
  if (!output) return;
  await run('Signing… If macOS asks for your PIN, enter it there.', async () => {
    const result = await invoke('sign_pdf', {
      input: state.path,
      output,
      field: name,
      identity: $('identity').value,
      newFields: newFields(),
    });
    state.lastOutput = result.output;
    await loadPdf(result.output, true);
    return `Signed ${name} at ${result.signed_at}.`;
  });
}

async function saveFields() {
  const count = state.added.length;
  const output = await save({
    defaultPath: state.path.replace(/(_fields)?\.pdf$/i, '_fields.pdf'),
    filters: pdfFilter,
  });
  if (!output) return;
  await run('Saving…', async () => {
    state.lastOutput = await invoke('save_fields', { input: state.path, output, newFields: newFields() });
    await loadPdf(state.lastOutput, true);
    return `Saved ${plural(count, 'new field')}.`;
  });
}

window.__TAURI__.webview.getCurrentWebview().onDragDropEvent(({ payload }) => {
  $('viewer').classList.toggle('hover', payload.type === 'enter' || payload.type === 'over');
  if (payload.type !== 'drop') return;
  const pdf = payload.paths.find((p) => p.toLowerCase().endsWith('.pdf'));
  if (pdf) {
    openPdf(pdf);
  } else {
    setStatus('Drop a PDF file.', 'error');
  }
});

$('choose').addEventListener('click', choosePdf);
$('choose-other').addEventListener('click', choosePdf);
$('refresh').addEventListener('click', loadIdentities);
$('identity').addEventListener('change', () => {
  describeIdentity();
  updateButtons();
});
$('sign').addEventListener('click', sign);
$('save-fields').addEventListener('click', saveFields);
$('reveal').addEventListener('click', () => invoke('reveal', { path: state.lastOutput }));

window.__TAURI__.event.listen('open-pdf', async () => {
  const path = await invoke('opened_file');
  if (path) openPdf(path);
});

loadIdentities();
invoke('opened_file').then((path) => path && openPdf(path));
