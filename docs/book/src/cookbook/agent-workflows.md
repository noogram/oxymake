# Agent-Driven Workflows

> Coming soon.

This page will demonstrate how AI agents can drive OxyMake pipelines
programmatically, covering:

- **Structured NDJSON events**: parsing `--json` output for typed event
  streams
- **Programmatic gate approval**: agents evaluating metrics and approving
  quality checkpoints via `ox gate approve`
- **Automated error recovery**: detecting failures from JSON events, adjusting
  parameters, and retrying
- **Multi-agent coordination**: multiple agents driving different stages of
  a pipeline
- **End-to-end example**: a complete pipeline driven by an LLM agent without
  human intervention
