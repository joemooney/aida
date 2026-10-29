'use strict';
// trace:TASK-b6b1278890774ef993d2ebc327862470-101 | ai:codex
const modes = {
  operator: { kicker:'HUMAN-LED DELIVERY', title:'Keep the work in human hands.', description:'AIDA presents the checklist and stops. A person performs the work; no implementation agent is launched by this execution mode.', example:'An operation requiring human credentials, physical access or direct judgment.', stages:[['Define intent & limits','Human'],['Perform the work','Human'],['Validate the outcome','Human'],['Complete the checklist','Human']], boundary:'The mode delegates no implementation to an agent. The human remains responsible for carrying out the work.' },
  guided: { kicker:'COLLABORATIVE DELIVERY', title:'Work through the decisions together.', description:'An agent works with a person in an interactive design session, records the design forks and prepares the change. The person stays involved in consequential choices.', example:'An architectural change with unresolved trade-offs that need a recorded decision.', stages:[['Resolve design forks','Human + agent'],['Implement & test','Agent'],['Review the resulting PR','Human'],['Authorize the merge','Human']], boundary:'Guided work does not auto-merge. Human participation is part of the execution path.' },
  drive: { kicker:'SUPERVISED DELIVERY', title:'Delegate the build. Keep the final call.', description:'Agents implement the change, open a pull request and run through CI. Execution stops there so a person can review and merge the result.', example:'A feature with clear implementation scope that still needs a human approval before merge.', stages:[['Define intent & limits','Human'],['Implement & open PR','Agent'],['Run through CI','Agent'],['Review & merge','Human']], boundary:'Drive stops after CI. It does not merge without the human taking the next step.' },
  drain: { kicker:'DARK FACTORY MODE', title:'Delegate the lifecycle.', description:'Agents implement, test, independently review and merge approved, authorized work. People set the intent, acceptance criteria and limits. Exceptions escalate.', example:'A bounded fix with clear acceptance criteria and no unresolved design decision.', stages:[['Define intent & limits','Human'],['Implement & test','Agent'],['Independent review','Agent'],['Merge through gates','Agent']], boundary:'Routine human coding and code review are removed from the execution path. Review, CI and authority gates remain.' }
};
const tabs = Array.from(document.querySelectorAll('[data-mode]'));
function chooseMode(key, focus = false) {
  const mode = modes[key];
  if (!mode) return;
  tabs.forEach(tab => { const selected = tab.dataset.mode === key; tab.setAttribute('aria-selected', String(selected)); tab.tabIndex = selected ? 0 : -1; if (selected && focus) tab.focus(); });
  document.getElementById('mode-panel').setAttribute('aria-labelledby', 'tab-' + key);
  for (const [id, value] of [['mode-kicker',mode.kicker],['mode-title',mode.title],['mode-description',mode.description],['mode-example',mode.example],['mode-boundary',mode.boundary]]) document.getElementById(id).textContent = value;
  const stages = document.getElementById('mode-stages');
  stages.replaceChildren(...mode.stages.map(([label, owner]) => { const item=document.createElement('li');const text=document.createElement('span');text.textContent=label;const badge=document.createElement('b');badge.textContent=owner;if(owner.includes('Human'))badge.className='human';item.append(text,badge);return item;}));
}
tabs.forEach((tab, index) => {
  tab.addEventListener('click', () => chooseMode(tab.dataset.mode));
  tab.addEventListener('keydown', event => { let next; if(event.key==='ArrowRight')next=(index+1)%tabs.length;else if(event.key==='ArrowLeft')next=(index+tabs.length-1)%tabs.length;else if(event.key==='Home')next=0;else if(event.key==='End')next=tabs.length-1;else return;event.preventDefault();chooseMode(tabs[next].dataset.mode,true); });
});
