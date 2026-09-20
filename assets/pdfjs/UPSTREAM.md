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
