'use strict';
// trace:TASK-B6B1278890774EF993D2EBC327862470-111 | ai:codex
const form = document.getElementById('library-search');
const query = document.getElementById('query');
const category = document.getElementById('category');
const cards = Array.from(document.querySelectorAll('.library-card'));
const records = new Map(window.AIDA_DOCS.map(entry => [entry.url, entry]));
function filterGuides() {
  const words = query.value.toLocaleLowerCase().trim().split(/\s+/).filter(Boolean);
  let count = 0;
  for (const card of cards) {
    const entry = records.get(card.dataset.url);
    const text = entry.text.toLocaleLowerCase();
    const matches = (category.value === 'all' || category.value === entry.category) && words.every(word => text.includes(word));
    card.hidden = !matches;
    if (matches) {
      count += 1;
      const match = words.length
        ? new RegExp(words[0].replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'iu').exec(entry.text)
        : null;
      const first = match ? Math.max(0, match.index - 45) : 0;
      card.querySelector('.excerpt').textContent = (first ? '…' : '') + entry.text.slice(first, first + 180) + '…';
    }
  }
  document.getElementById('search-status').textContent = `${count} matching ${count === 1 ? 'guide' : 'guides'}.`;
  document.getElementById('search-empty').hidden = count !== 0;
}
form.addEventListener('submit', event => { event.preventDefault(); filterGuides(); });
query.addEventListener('input', filterGuides);
category.addEventListener('change', filterGuides);
form.addEventListener('reset', () => { setTimeout(filterGuides, 0); });
filterGuides();
