# Proveniência do PDF.js vendorizado

`pdf.mjs` e `pdf.worker.mjs` são do PDF.js, copiados sem alterações. Os outros
ficheiros desta pasta (`viewer.html`, `viewer.mjs`) são do NeuralIA e não têm
upstream.

| Campo | Valor |
|---|---|
| Projeto | Mozilla PDF.js |
| Versão | **6.3.289** |
| Origem | https://github.com/mozilla/pdf.js — release `v6.3.289`, build `pdfjs-6.3.289-dist.zip` |
| Licença | Apache-2.0 (`LICENSE`, nesta pasta) |
| Alterações locais | nenhuma |

## Ficheiros

| Ficheiro | SHA-256 |
|---|---|
| `pdf.mjs` | `df4a14ea8a67f687265d7d51d89cc4fce429739f6e8076dea163b164024fe6e2` |
| `pdf.worker.mjs` | `f2870db902eaff8397442c912b69459980ac91f6f4b5ed827167b12cf7057930` |
| `LICENSE` | `0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594` |

## Porque é que isto está preso por um teste

O `release.yml` acrescenta ao SBOM (CycloneDX) um componente `pdf.js` com a
versão escrita à mão no próprio workflow. Sem nada a ligar as duas pontas, uma
atualização do `pdf.mjs` deixava o SBOM publicado a declarar uma versão que já
não é a que embarca — uma afirmação falsa num artefacto de release, que é o
sítio onde ela custa mais.

`crates/neural-core/tests/vendored_provenance.rs` compara três coisas que têm de
dizer o mesmo número: a `const version` dentro do `pdf.mjs`, a tabela acima, e a
versão escrita no `release.yml`. Atualizar o PDF.js implica atualizar este
ficheiro — é esse o objetivo.

## Assets auxiliares do viewer

A política do NeuralIA para o PDF.js 6.3.289 é **suporte integral e offline**. Todos os assets abaixo vêm do mesmo commit upstream `1c8020a7d4e43668ac287a3ecf9a8dbea17e4c56` da tag `v6.3.289` e são servidos somente pela origem privada `http://neuralia-pdf.localhost`, que o protocolo personalizado `neuralia-pdf` do Wry intercepta, através de uma allowlist fechada.

- `cmaps/`: 168 Adobe binary CMaps (`.bcmap`), licença BSD-style Adobe.
- `standard_fonts/`: 10 fontes Foxit/PDFium em PFB + 4 Liberation Sans 1.07.4 em TTF. Foxit: BSD-style. Liberation Sans 1.07.4: GPLv2 + Liberation font exception.
- `wasm/`: OpenJPEG, JBIG2 e QCMS, incluindo os fallbacks JavaScript distribuídos pelo PDF.js quando aplicável. OpenJPEG: BSD-2-Clause; JBIG2: preservar as licenças upstream incluídas; QCMS: MIT.
- `icc/`: `CGATS001Compat-v2-micro.icc`, CC0-1.0, proveniente de Compact-ICC-Profiles revision `bdd84663061bc4ae95ca70decff54f581e27f702` conforme o README upstream do PDF.js.
- `fixtures/`: PDFs de teste upstream usados apenas pelos gates de aceitação do viewer.

O mapeamento completo de destino, caminho upstream, tamanho e Git blob SHA-1 está em `AUXILIARY_BLOBS.md`. Esse manifesto é parte da prova de proveniência: atualizar qualquer família de assets exige atualizar o manifesto e os gates.
