# SPEC-0110 — Leitura textual de PDF

**Status:** Implementada após gate verde.

## Objetivo

O visualizador PDF já renderiza documentos offline e possui TextLayer para seleção e leitura em voz alta. Esta fase reutiliza essa infraestrutura para colocar o conteúdo real do PDF na memória semântica do NeuralIA, por página, sem OCR.

## Limites

A extração automática é limitada a:

- 24 páginas por documento;
- 1.500 caracteres por página no IPC;
- 30.000 caracteres no total extraído pelo viewer.

O parser nativo rejeita página zero, página acima do limite, texto vazio e payload acima do limite. O event loop só aceita o conteúdo enquanto a superfície ativa continua sendo o PDF.

## Segurança

O viewer recebe somente `__neuralia_pdf_page_text(page, text)`, protegido pela capability da SPEC-0108. Não recebe um dispatcher genérico.

A ação `pdf-page-text` existe no parser fechado do IPC, mas somente o builder do PDF a traduz para `UserEvent::PdfPageText`; nas demais WebViews ela cai no caminho comum e não produz evento.

## Memória

Cada página com texto vira um `MemoryDocument` com `MemorySourceKind::Pdf`, URL original e título `<arquivo> · página N`. A gravação passa pelo `PrivacyGuard`, preservando as regras de persistência e modo privado atuais.

O registo inicial de “PDF aberto” continua existindo; as páginas acrescentam conteúdo pesquisável real.

## Texto selecionável

A TextLayer já é parte do viewer atual e é compartilhada com a leitura em voz alta. Esta fase não cria uma segunda camada: usa o mesmo `getTextContent()` do PDF.js e mantém o ciclo de vida existente de canvas/TextLayer.

## Fora de escopo

Não há OCR. PDFs compostos apenas por imagens continuam visíveis, mas não geram texto pesquisável.

## Gates

- `pdf_page_text_is_capability_authenticated_and_bounded`: autenticação, página e teto de texto;
- `pdf_text_bridge_is_specific_and_bounded`: ponte específica, sem dispatcher genérico;
- `test-pdf-viewer.mjs`: ordem de leitura, quebras de linha e corte duro;
- CI completo da branch de integração.
