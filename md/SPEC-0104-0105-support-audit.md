# Auditoria de apoio — SPEC-0104 / SPEC-0105

**Data:** 19/09/2026  
**Base funcional auditada:** `main@a5391f9`  
**Compatibilidade verificada:** os blocos de agente relevantes em `windows_app.rs` são idênticos no candidato `feat/astra-finish-audit-2.0@3ec01dd`.  
**Natureza:** auditoria de apoio. **Não conta como revisão adversarial independente**, porque Astra participa da construção do produto.

Este documento não altera código, status de spec ou release. O objetivo é reduzir o trabalho do revisor independente e registrar gaps reproduzíveis.

## Resumo executivo

A implementação tem uma boa base de política nativa, mas hoje existem **dois runtimes de agente diferentes**:

1. `neural-core::AgentRuntime`, coberto pelos testes de SPEC-0105;
2. `BrowserAgentState` em `windows_app.rs`, que é o caminho realmente usado pelo binário.

Essa duplicação cria divergências entre aquilo que os testes de core provam e aquilo que o produto realmente executa.

A auditoria encontrou um achado crítico, quatro altos e vários médios.

---

## CRITICAL-01 — `save_agent_outcome` pode persistir senha/OTP/cartão em claro

### Evidência

`AgentOutcome::NeedsApproval` contém:

```rust
NeedsApproval {
    action: AgentAction,
    decision: PolicyDecision,
    trace: Vec<AgentStep>,
}
```

`AgentAction::TypeText` contém o campo `text: String`.

Para `FieldKind::Password`, `Otp` ou `PaymentCard`, a policy corretamente classifica a ação como `Restricted`, mas o runtime retorna `NeedsApproval` **com a ação original intacta**.

`save_agent_outcome()` serializa o outcome inteiro com Serde.

Resultado: um chamador pode persistir o segredo que a própria policy deveria impedir de entrar no audit trail.

O teste atual `audit_log_can_be_persisted_without_secret_values` cobre apenas `AgentPermissionPolicy::write_audit_log`, não `save_agent_outcome`.

### Reprodução que deve virar teste

```rust
let password = AgentAction::TypeText {
    target,
    text: "hunter2".into(),
    field: FieldKind::Password,
};

let outcome = runtime.run("login", page);
save_agent_outcome(path, "login", &outcome)?;

assert!(!fs::read_to_string(path)?.contains("hunter2"));
```

**Estado atual esperado:** o assert falha.

### Correção recomendada

Nunca serializar `AgentAction` sensível com valor bruto.

Opções seguras:

- criar um `PersistedAgentAction` redigido;
- customizar `Serialize` para `AgentAction::TypeText`;
- ou guardar somente `field`, comprimento e fingerprint não reversível.

Senha, OTP, cartão e qualquer valor classificado sensível nunca entram em `AgentOutcome` persistido.

---

## HIGH-01 — o planner recebe `ObservedPage` sem sanitização obrigatória

### Evidência

`AgentRuntime::run` chama diretamente:

```rust
self.planner.plan(goal, &page, &self.trace)
```

Não há chamada a `redact_sensitive_text` nem uma estrutura `SanitizedObservedPage`.

A SPEC-0105 §6 diz que o planner recebe a observação sanitizada; a SPEC-0104 §5 exige firewall de dados sensíveis antes de contexto de modelo.

No binário atual, o BrowserAgent é determinístico e não usa um LLM planner. Porém `AgentRuntime` é API pública e model-independent, então a fronteira precisa existir no core, não depender do chamador lembrar dela.

### Teste proposto

Um planner de teste captura `page.text_excerpt`. O input inclui:

```text
Authorization: Bearer abc
password=hunter2
conteúdo visível
```

O planner deve receber apenas a versão sanitizada.

### Correção recomendada

Introduzir uma fronteira explícita:

```text
ObservedPage
  -> sanitize_observation()
  -> PlannerObservation
  -> AgentPlanner
```

O tipo entregue ao planner não deve sequer possuir campos que podem conter valores proibidos sem redaction.

---

## HIGH-02 — referência de elemento não é autenticada contra a observação atual

### Evidência

`validate_action_generation` verifica apenas:

```text
target.generation == page.generation
```

Não verifica:

- se o `id` está em `page.elements`;
- se `origin` e `frame` coincidem;
- se o elemento observado continua `visible`/`interactable`;
- se o planner fabricou um `AgentElement` inteiro.

O planner recebe `ObservedPage`, mas retorna `AgentAction` contendo o objeto `AgentElement` completo. Isso permite fabricar uma referência com generation atual.

A SPEC-0105 diz que planner deve referenciar IDs curtos e efêmeros, não construir metadados de autoridade.

### Correção recomendada

O planner retorna apenas um handle opaco:

```rust
ElementRef {
    id,
    generation,
}
```

O runtime resolve o handle contra a observação nativa e copia internamente:

- origin;
- frame;
- role;
- visibility;
- interactability.

Nenhum desses campos deve vir do planner.

### Testes propostos

- ID inexistente com generation correto → rejeitado;
- ID existente com origin adulterada → rejeitado;
- elemento não interagível → rejeitado;
- frame adulterado → rejeitado.

---

## HIGH-03 — grant reversível sobrevive a mudança de origem no wiring real

### Evidência

`AgentPermissionPolicy` possui:

```rust
session_reversible_grant: bool
```

O grant não é escopado por origem.

`evaluate_live` verifica `approved_origins` apenas para `Navigate`.

Para `Click` e `TypeText`, um grant de sessão permite a ação independentemente do origin informado.

No binário, `start_browser_agent` faz:

```rust
policy.grant_reversible_session_actions(true);
```

O WebView continua podendo navegar/redirect para outra origem pública. Quando a nova página gera uma observação, `app_agent_security_action` usa a nova origem, mas a policy mantém o mesmo grant booleano.

Logo uma sequência iniciada em `https://a.example` pode, após redirect para `https://b.example`, continuar clicando/digitando ações reversíveis sem uma nova decisão.

Isso contradiz SPEC-0104 §6: mudança de origem pode reduzir capabilities.

### Correção recomendada

Grants devem ser escopados:

```text
(origin, capability-class, run-id)
```

Mudança de origem:

- revoga grant reversível por padrão;
- ou exige `approve_origin` explícito;
- só então reativa classes permitidas.

### Teste proposto

1. grant em `a.example`;
2. observação seguinte em `b.example`;
3. Click em `b.example`;
4. decisão deve exigir nova confirmação.

---

## HIGH-04 — o binário não usa `neural-core::AgentRuntime`

### Evidência

Busca por `AgentRuntime::new` encontra somente:

- testes dentro de `agent_runtime.rs`;
- `spec_010x_acceptance.rs`.

`windows_app.rs` implementa outra máquina de estados:

- `BrowserAgentState`;
- hard limit 24 steps;
- hard limit 120 s;
- classificação própria `app_agent_security_action`;
- execução própria `agent_action_script`.

Portanto o teste SPEC-0105 do core **não prova o caminho de produção**.

### Correção recomendada

Uma das duas estratégias precisa vencer:

**A. Preferida:** fazer o browser wiring usar `AgentRuntime` com adapters de planner/executor.

**B. Alternativa:** declarar o core runtime como biblioteca experimental e criar acceptance tests reais do `BrowserAgentState`/wiring.

Manter dois runtimes de segurança paralelos é custo de auditoria permanente.

---

## HIGH-05 — classificação de click sensível depende de palavras no label

### Evidência

`app_agent_security_action` classifica click como:

- delete: `delete/remove/excluir/apagar`;
- payment: `buy/purchase/pay/comprar/pagar`;
- submit: `send/submit/confirm/enviar/confirmar`;
- qualquer outro texto → `Click` reversível.

Como `session_reversible_grant=true`, labels como estes podem passar sem confirmação:

- `Post`;
- `Save changes`;
- `Create account`;
- `Authorize`;
- `Transfer`;
- `Checkout`;
- `Subscribe`;
- `Accept terms`;
- `Sign agreement`;
- `Continue` em uma etapa final.

A sensibilidade de uma ação não pode ser inferida apenas por substring do botão.

### Correção recomendada

A bridge deve produzir intenção estrutural, por exemplo:

- submit de form;
- button type=submit;
- form/action;
- surrounding labels;
- field categories presentes;
- semantic command explícito.

Na dúvida, cair para classe mais restritiva, nunca mais permissiva.

---

## MEDIUM-01 — wall-time não é hard deadline

O relógio é checado antes de cada iteração.

Se `planner.plan()` ou `executor.execute()` bloquear por 5 minutos, um `max_wall_time=120s` não o interrompe.

A claim correta hoje é: **budget verificado entre etapas síncronas**.

Para hard deadline real, planner/executor precisam de cancelamento/cooperative deadline ou worker isolado.

---

## MEDIUM-02 — kill switch genérico só é observado entre etapas

`policy.stopped()` é consultado no início da iteração.

Não interrompe uma chamada síncrona já em curso.

No wiring Windows, Escape/Home destrói a superfície WebView e remove `active_agent`, o que é melhor. A SPEC precisa deixar claro qual caminho garante interrupção real.

---

## MEDIUM-03 — approval fingerprint e approve_origin não estão ligados ao fluxo

`approval_fingerprint()` não possui call site fora da própria definição.

`approve_origin()` também não aparece no produto.

Isso sugere que a intenção de vincular aprovação à ação/origem existe, mas ainda não fecha o ciclo.

O approval deveria ser associado a:

- run-id;
- action fingerprint;
- current origin;
- observation generation;
- expiração curta.

---

## MEDIUM-04 — observador é top-frame only

`parse_agent_observation` grava:

```rust
frame: "top"
```

e o script consulta o DOM do documento corrente.

Não há observação real de nested iframes no caminho de produção.

Os fixtures do PR #11 são úteis para provar que **uma observação hostil já construída** não concede capabilities, mas não provam a cadeia:

```text
raw DOM / iframe / PDF -> observation -> policy
```

Esse gap deve continuar explicitamente registrado.

---

## MEDIUM-05 — traces do app guardam goal/action em texto bruto

`finish_agent` persiste:

- `goal`;
- `format!("{action:?}")`.

Hoje os comandos expostos são limitados e não incluem senha/OTP, então o impacto real é menor que CRITICAL-01.

Mesmo assim, a regra de audit trail deve ser única: qualquer persistência de trace usa uma representação redigida.

---

# Matriz de aceitação a adicionar depois do unfreeze

## SPEC-0104

- [ ] password/OTP/card nunca aparecem em `save_agent_outcome`;
- [ ] planner nunca recebe segredo de uma `ObservedPage`;
- [ ] elemento fabricado pelo planner é rejeitado;
- [ ] grant reversível é revogado ao mudar de origem;
- [ ] sensitive click sem keyword explícita ainda é classificado de forma segura;
- [ ] approval está vinculado a action+origin+generation;
- [ ] cross-origin redirect não herda capabilities;
- [ ] fixtures do PR #11 continuam verdes;
- [ ] teste end-to-end DOM/iframe -> observation -> policy.

## SPEC-0105

- [ ] caminho de produção usa o mesmo runtime coberto pelo acceptance gate, ou possui acceptance equivalente;
- [ ] max wall time tem semântica documentada e testada;
- [ ] kill switch interrompe trabalho em voo onde tecnicamente possível;
- [ ] stale element valida ID/membership, não apenas generation;
- [ ] trace persistido é redigido;
- [ ] nenhum executor aceita JavaScript arbitrário oriundo do planner.

# Ordem de correção sugerida

1. CRITICAL-01: persistência de segredo;
2. HIGH-03: grant cross-origin;
3. HIGH-05: classificação de ação sensível;
4. HIGH-02: referência opaca/autenticada;
5. HIGH-01: sanitização tipada antes do planner;
6. HIGH-04: unificar runtime/wiring;
7. deadlines/kill/trace hardening.

# Consequência para status

Este relatório **não altera sozinho** as specs.

Mas a revisão independente deve tratar os achados acima antes de declarar 0104/0105 auditadas. Em especial, CRITICAL-01 não deve ser aceito como dívida cosmética.

A decisão de versão/release continua com o dono + auditor conforme `AGENTS.md`.
