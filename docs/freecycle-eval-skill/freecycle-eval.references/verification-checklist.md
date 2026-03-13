# Verification Checklist

Run this checklist before considering the FreeCycle evaluation skill complete.

## 1. Clarity Auditor

- Are the discovery questions unambiguous?
- Is workload detection clearly defined?
- Is the evaluation framework clear?
- Could a user misinterpret any instruction?

## 2. Role and Context Reviewer

- Does the skill properly contextualize the evaluation?
- Does it avoid assuming user requirements that were not explicitly gathered?
- Does it explain FreeCycle-specific concepts such as cooldown, wake delay, and remote install unlocks?
- Is the installation requirement clear?

## 3. Structure Inspector

- Is the output well organized?
- Can the user jump to a specific section quickly?
- Is the flow logical from discovery through final recommendation?
- Is the main skill lean enough that large references are loaded only when needed?

## 4. Examples Specialist

- Are there concrete examples of when to use each option?
- Are the tool call examples realistic and current?
- Do the examples show both single-model and multi-model workflows when appropriate?
- Do the examples include static persistent code patterns where that is the better engineering choice?

## 5. Negative Constraints Guardian

- Does the skill specify what not to do?
- Are privacy rules tied to explicit user answers instead of hidden assumptions?
- Are remote-install, wake-on-LAN, and availability failure modes covered?
- Does the skill warn against relying on `freecycle_evaluate_task` alone?

## 6. Reasoning Validator

- Does it walk through the decision tree step by step?
- Is the scoring methodology transparent?
- Can the user understand why a recommendation was made?
- Does it explain when multi-stage routing is worth the complexity?

## 7. Output Format Checker

- Is the final recommendation structured and actionable?
- Does it produce a per-stage routing plan when the workflow is multi-stage?
- Does it include a model-strategy note that compares simpler and more complex options?
- Are the sections parseable if another tool needs to consume them?

## 8. Adversarial Tester

Check these edge cases explicitly:

- FreeCycle is down
- local model is too slow
- user wants real-time latency with weak hardware
- GPU is frequently blocked by games
- MCP host points at the wrong IP
- no installed model is a clear fit
- multiple different stages want different models
- a static code path would be cheaper than repeated tool or cloud calls
