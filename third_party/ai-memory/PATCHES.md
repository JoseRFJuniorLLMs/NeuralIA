# NeuralIA adaptations from ai-memory

NeuralIA preserves upstream MIT attribution while deliberately diverging from
ai-memory's coding-agent product model.

- Browser-native domain types replace project/workstream/harness concepts.
- Maintained knowledge is file-backed; SQLite is a rebuildable derived index.
- Retrieval combines lexical, entity, graph and local semantic signals.
- Local intelligence is optional/lazy and native Home does not require a model.
- Private/incognito navigation is rejected before persistence.
- No mandatory HTTP/MCP server, Docker stack or background daemon.
- Agent execution remains behind NeuralIA's native permission/origin policy.
- Upstream updates are reviewed and selectively ported, never merged wholesale.
