//! Assets auxiliares do PDF.js 6.3.289 embutidos no binário.
//!
//! A lookup é uma allowlist exata: nenhum caminho vindo do WebView é convertido
//! em caminho de disco e sequências como ".." nunca podem escapar da árvore.

pub(crate) fn lookup(path: &str) -> Option<(&'static str, &'static [u8])> {
    Some(match path {
        "/cmaps/78-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/78-EUC-H.bcmap"),
        ),
        "/cmaps/78-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/78-EUC-V.bcmap"),
        ),
        "/cmaps/78-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/78-H.bcmap"),
        ),
        "/cmaps/78-RKSJ-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/78-RKSJ-H.bcmap"),
        ),
        "/cmaps/78-RKSJ-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/78-RKSJ-V.bcmap"),
        ),
        "/cmaps/78-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/78-V.bcmap"),
        ),
        "/cmaps/78ms-RKSJ-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/78ms-RKSJ-H.bcmap"),
        ),
        "/cmaps/78ms-RKSJ-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/78ms-RKSJ-V.bcmap"),
        ),
        "/cmaps/83pv-RKSJ-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/83pv-RKSJ-H.bcmap"),
        ),
        "/cmaps/90ms-RKSJ-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/90ms-RKSJ-H.bcmap"),
        ),
        "/cmaps/90ms-RKSJ-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/90ms-RKSJ-V.bcmap"),
        ),
        "/cmaps/90msp-RKSJ-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/90msp-RKSJ-H.bcmap"),
        ),
        "/cmaps/90msp-RKSJ-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/90msp-RKSJ-V.bcmap"),
        ),
        "/cmaps/90pv-RKSJ-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/90pv-RKSJ-H.bcmap"),
        ),
        "/cmaps/90pv-RKSJ-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/90pv-RKSJ-V.bcmap"),
        ),
        "/cmaps/Add-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Add-H.bcmap"),
        ),
        "/cmaps/Add-RKSJ-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Add-RKSJ-H.bcmap"),
        ),
        "/cmaps/Add-RKSJ-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Add-RKSJ-V.bcmap"),
        ),
        "/cmaps/Add-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Add-V.bcmap"),
        ),
        "/cmaps/Adobe-CNS1-0.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-CNS1-0.bcmap"),
        ),
        "/cmaps/Adobe-CNS1-1.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-CNS1-1.bcmap"),
        ),
        "/cmaps/Adobe-CNS1-2.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-CNS1-2.bcmap"),
        ),
        "/cmaps/Adobe-CNS1-3.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-CNS1-3.bcmap"),
        ),
        "/cmaps/Adobe-CNS1-4.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-CNS1-4.bcmap"),
        ),
        "/cmaps/Adobe-CNS1-5.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-CNS1-5.bcmap"),
        ),
        "/cmaps/Adobe-CNS1-6.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-CNS1-6.bcmap"),
        ),
        "/cmaps/Adobe-CNS1-UCS2.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-CNS1-UCS2.bcmap"),
        ),
        "/cmaps/Adobe-GB1-0.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-GB1-0.bcmap"),
        ),
        "/cmaps/Adobe-GB1-1.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-GB1-1.bcmap"),
        ),
        "/cmaps/Adobe-GB1-2.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-GB1-2.bcmap"),
        ),
        "/cmaps/Adobe-GB1-3.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-GB1-3.bcmap"),
        ),
        "/cmaps/Adobe-GB1-4.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-GB1-4.bcmap"),
        ),
        "/cmaps/Adobe-GB1-5.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-GB1-5.bcmap"),
        ),
        "/cmaps/Adobe-GB1-UCS2.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-GB1-UCS2.bcmap"),
        ),
        "/cmaps/Adobe-Japan1-0.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Japan1-0.bcmap"),
        ),
        "/cmaps/Adobe-Japan1-1.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Japan1-1.bcmap"),
        ),
        "/cmaps/Adobe-Japan1-2.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Japan1-2.bcmap"),
        ),
        "/cmaps/Adobe-Japan1-3.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Japan1-3.bcmap"),
        ),
        "/cmaps/Adobe-Japan1-4.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Japan1-4.bcmap"),
        ),
        "/cmaps/Adobe-Japan1-5.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Japan1-5.bcmap"),
        ),
        "/cmaps/Adobe-Japan1-6.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Japan1-6.bcmap"),
        ),
        "/cmaps/Adobe-Japan1-UCS2.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Japan1-UCS2.bcmap"),
        ),
        "/cmaps/Adobe-Korea1-0.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Korea1-0.bcmap"),
        ),
        "/cmaps/Adobe-Korea1-1.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Korea1-1.bcmap"),
        ),
        "/cmaps/Adobe-Korea1-2.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Korea1-2.bcmap"),
        ),
        "/cmaps/Adobe-Korea1-UCS2.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Adobe-Korea1-UCS2.bcmap"),
        ),
        "/cmaps/B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/B5-H.bcmap"),
        ),
        "/cmaps/B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/B5-V.bcmap"),
        ),
        "/cmaps/B5pc-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/B5pc-H.bcmap"),
        ),
        "/cmaps/B5pc-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/B5pc-V.bcmap"),
        ),
        "/cmaps/CNS-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/CNS-EUC-H.bcmap"),
        ),
        "/cmaps/CNS-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/CNS-EUC-V.bcmap"),
        ),
        "/cmaps/CNS1-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/CNS1-H.bcmap"),
        ),
        "/cmaps/CNS1-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/CNS1-V.bcmap"),
        ),
        "/cmaps/CNS2-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/CNS2-H.bcmap"),
        ),
        "/cmaps/CNS2-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/CNS2-V.bcmap"),
        ),
        "/cmaps/ETHK-B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/ETHK-B5-H.bcmap"),
        ),
        "/cmaps/ETHK-B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/ETHK-B5-V.bcmap"),
        ),
        "/cmaps/ETen-B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/ETen-B5-H.bcmap"),
        ),
        "/cmaps/ETen-B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/ETen-B5-V.bcmap"),
        ),
        "/cmaps/ETenms-B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/ETenms-B5-H.bcmap"),
        ),
        "/cmaps/ETenms-B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/ETenms-B5-V.bcmap"),
        ),
        "/cmaps/EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/EUC-H.bcmap"),
        ),
        "/cmaps/EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/EUC-V.bcmap"),
        ),
        "/cmaps/Ext-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Ext-H.bcmap"),
        ),
        "/cmaps/Ext-RKSJ-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Ext-RKSJ-H.bcmap"),
        ),
        "/cmaps/Ext-RKSJ-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Ext-RKSJ-V.bcmap"),
        ),
        "/cmaps/Ext-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Ext-V.bcmap"),
        ),
        "/cmaps/GB-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GB-EUC-H.bcmap"),
        ),
        "/cmaps/GB-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GB-EUC-V.bcmap"),
        ),
        "/cmaps/GB-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GB-H.bcmap"),
        ),
        "/cmaps/GB-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GB-V.bcmap"),
        ),
        "/cmaps/GBK-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBK-EUC-H.bcmap"),
        ),
        "/cmaps/GBK-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBK-EUC-V.bcmap"),
        ),
        "/cmaps/GBK2K-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBK2K-H.bcmap"),
        ),
        "/cmaps/GBK2K-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBK2K-V.bcmap"),
        ),
        "/cmaps/GBKp-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBKp-EUC-H.bcmap"),
        ),
        "/cmaps/GBKp-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBKp-EUC-V.bcmap"),
        ),
        "/cmaps/GBT-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBT-EUC-H.bcmap"),
        ),
        "/cmaps/GBT-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBT-EUC-V.bcmap"),
        ),
        "/cmaps/GBT-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBT-H.bcmap"),
        ),
        "/cmaps/GBT-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBT-V.bcmap"),
        ),
        "/cmaps/GBTpc-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBTpc-EUC-H.bcmap"),
        ),
        "/cmaps/GBTpc-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBTpc-EUC-V.bcmap"),
        ),
        "/cmaps/GBpc-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBpc-EUC-H.bcmap"),
        ),
        "/cmaps/GBpc-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/GBpc-EUC-V.bcmap"),
        ),
        "/cmaps/H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/H.bcmap"),
        ),
        "/cmaps/HKdla-B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKdla-B5-H.bcmap"),
        ),
        "/cmaps/HKdla-B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKdla-B5-V.bcmap"),
        ),
        "/cmaps/HKdlb-B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKdlb-B5-H.bcmap"),
        ),
        "/cmaps/HKdlb-B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKdlb-B5-V.bcmap"),
        ),
        "/cmaps/HKgccs-B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKgccs-B5-H.bcmap"),
        ),
        "/cmaps/HKgccs-B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKgccs-B5-V.bcmap"),
        ),
        "/cmaps/HKm314-B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKm314-B5-H.bcmap"),
        ),
        "/cmaps/HKm314-B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKm314-B5-V.bcmap"),
        ),
        "/cmaps/HKm471-B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKm471-B5-H.bcmap"),
        ),
        "/cmaps/HKm471-B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKm471-B5-V.bcmap"),
        ),
        "/cmaps/HKscs-B5-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKscs-B5-H.bcmap"),
        ),
        "/cmaps/HKscs-B5-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/HKscs-B5-V.bcmap"),
        ),
        "/cmaps/Hankaku.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Hankaku.bcmap"),
        ),
        "/cmaps/Hiragana.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Hiragana.bcmap"),
        ),
        "/cmaps/KSC-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSC-EUC-H.bcmap"),
        ),
        "/cmaps/KSC-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSC-EUC-V.bcmap"),
        ),
        "/cmaps/KSC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSC-H.bcmap"),
        ),
        "/cmaps/KSC-Johab-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSC-Johab-H.bcmap"),
        ),
        "/cmaps/KSC-Johab-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSC-Johab-V.bcmap"),
        ),
        "/cmaps/KSC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSC-V.bcmap"),
        ),
        "/cmaps/KSCms-UHC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSCms-UHC-H.bcmap"),
        ),
        "/cmaps/KSCms-UHC-HW-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSCms-UHC-HW-H.bcmap"),
        ),
        "/cmaps/KSCms-UHC-HW-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSCms-UHC-HW-V.bcmap"),
        ),
        "/cmaps/KSCms-UHC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSCms-UHC-V.bcmap"),
        ),
        "/cmaps/KSCpc-EUC-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSCpc-EUC-H.bcmap"),
        ),
        "/cmaps/KSCpc-EUC-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/KSCpc-EUC-V.bcmap"),
        ),
        "/cmaps/Katakana.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Katakana.bcmap"),
        ),
        "/cmaps/NWP-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/NWP-H.bcmap"),
        ),
        "/cmaps/NWP-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/NWP-V.bcmap"),
        ),
        "/cmaps/RKSJ-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/RKSJ-H.bcmap"),
        ),
        "/cmaps/RKSJ-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/RKSJ-V.bcmap"),
        ),
        "/cmaps/Roman.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/Roman.bcmap"),
        ),
        "/cmaps/UniCNS-UCS2-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniCNS-UCS2-H.bcmap"),
        ),
        "/cmaps/UniCNS-UCS2-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniCNS-UCS2-V.bcmap"),
        ),
        "/cmaps/UniCNS-UTF16-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniCNS-UTF16-H.bcmap"),
        ),
        "/cmaps/UniCNS-UTF16-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniCNS-UTF16-V.bcmap"),
        ),
        "/cmaps/UniCNS-UTF32-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniCNS-UTF32-H.bcmap"),
        ),
        "/cmaps/UniCNS-UTF32-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniCNS-UTF32-V.bcmap"),
        ),
        "/cmaps/UniCNS-UTF8-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniCNS-UTF8-H.bcmap"),
        ),
        "/cmaps/UniCNS-UTF8-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniCNS-UTF8-V.bcmap"),
        ),
        "/cmaps/UniGB-UCS2-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniGB-UCS2-H.bcmap"),
        ),
        "/cmaps/UniGB-UCS2-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniGB-UCS2-V.bcmap"),
        ),
        "/cmaps/UniGB-UTF16-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniGB-UTF16-H.bcmap"),
        ),
        "/cmaps/UniGB-UTF16-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniGB-UTF16-V.bcmap"),
        ),
        "/cmaps/UniGB-UTF32-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniGB-UTF32-H.bcmap"),
        ),
        "/cmaps/UniGB-UTF32-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniGB-UTF32-V.bcmap"),
        ),
        "/cmaps/UniGB-UTF8-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniGB-UTF8-H.bcmap"),
        ),
        "/cmaps/UniGB-UTF8-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniGB-UTF8-V.bcmap"),
        ),
        "/cmaps/UniJIS-UCS2-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UCS2-H.bcmap"),
        ),
        "/cmaps/UniJIS-UCS2-HW-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UCS2-HW-H.bcmap"),
        ),
        "/cmaps/UniJIS-UCS2-HW-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UCS2-HW-V.bcmap"),
        ),
        "/cmaps/UniJIS-UCS2-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UCS2-V.bcmap"),
        ),
        "/cmaps/UniJIS-UTF16-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UTF16-H.bcmap"),
        ),
        "/cmaps/UniJIS-UTF16-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UTF16-V.bcmap"),
        ),
        "/cmaps/UniJIS-UTF32-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UTF32-H.bcmap"),
        ),
        "/cmaps/UniJIS-UTF32-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UTF32-V.bcmap"),
        ),
        "/cmaps/UniJIS-UTF8-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UTF8-H.bcmap"),
        ),
        "/cmaps/UniJIS-UTF8-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS-UTF8-V.bcmap"),
        ),
        "/cmaps/UniJIS2004-UTF16-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS2004-UTF16-H.bcmap"),
        ),
        "/cmaps/UniJIS2004-UTF16-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS2004-UTF16-V.bcmap"),
        ),
        "/cmaps/UniJIS2004-UTF32-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS2004-UTF32-H.bcmap"),
        ),
        "/cmaps/UniJIS2004-UTF32-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS2004-UTF32-V.bcmap"),
        ),
        "/cmaps/UniJIS2004-UTF8-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS2004-UTF8-H.bcmap"),
        ),
        "/cmaps/UniJIS2004-UTF8-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJIS2004-UTF8-V.bcmap"),
        ),
        "/cmaps/UniJISPro-UCS2-HW-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJISPro-UCS2-HW-V.bcmap"),
        ),
        "/cmaps/UniJISPro-UCS2-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJISPro-UCS2-V.bcmap"),
        ),
        "/cmaps/UniJISPro-UTF8-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJISPro-UTF8-V.bcmap"),
        ),
        "/cmaps/UniJISX0213-UTF32-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJISX0213-UTF32-H.bcmap"),
        ),
        "/cmaps/UniJISX0213-UTF32-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJISX0213-UTF32-V.bcmap"),
        ),
        "/cmaps/UniJISX02132004-UTF32-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJISX02132004-UTF32-H.bcmap"),
        ),
        "/cmaps/UniJISX02132004-UTF32-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniJISX02132004-UTF32-V.bcmap"),
        ),
        "/cmaps/UniKS-UCS2-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniKS-UCS2-H.bcmap"),
        ),
        "/cmaps/UniKS-UCS2-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniKS-UCS2-V.bcmap"),
        ),
        "/cmaps/UniKS-UTF16-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniKS-UTF16-H.bcmap"),
        ),
        "/cmaps/UniKS-UTF16-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniKS-UTF16-V.bcmap"),
        ),
        "/cmaps/UniKS-UTF32-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniKS-UTF32-H.bcmap"),
        ),
        "/cmaps/UniKS-UTF32-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniKS-UTF32-V.bcmap"),
        ),
        "/cmaps/UniKS-UTF8-H.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniKS-UTF8-H.bcmap"),
        ),
        "/cmaps/UniKS-UTF8-V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/UniKS-UTF8-V.bcmap"),
        ),
        "/cmaps/V.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/V.bcmap"),
        ),
        "/cmaps/WP-Symbol.bcmap" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/cmaps/WP-Symbol.bcmap"),
        ),
        "/icc/CGATS001Compat-v2-micro.icc" => (
            "application/vnd.iccprofile",
            include_bytes!("../../../assets/pdfjs/icc/CGATS001Compat-v2-micro.icc"),
        ),
        "/standard_fonts/FoxitDingbats.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitDingbats.pfb"),
        ),
        "/standard_fonts/FoxitFixed.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitFixed.pfb"),
        ),
        "/standard_fonts/FoxitFixedBold.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitFixedBold.pfb"),
        ),
        "/standard_fonts/FoxitFixedBoldItalic.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitFixedBoldItalic.pfb"),
        ),
        "/standard_fonts/FoxitFixedItalic.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitFixedItalic.pfb"),
        ),
        "/standard_fonts/FoxitSerif.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitSerif.pfb"),
        ),
        "/standard_fonts/FoxitSerifBold.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitSerifBold.pfb"),
        ),
        "/standard_fonts/FoxitSerifBoldItalic.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitSerifBoldItalic.pfb"),
        ),
        "/standard_fonts/FoxitSerifItalic.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitSerifItalic.pfb"),
        ),
        "/standard_fonts/FoxitSymbol.pfb" => (
            "application/octet-stream",
            include_bytes!("../../../assets/pdfjs/standard_fonts/FoxitSymbol.pfb"),
        ),
        "/standard_fonts/LiberationSans-Bold.ttf" => (
            "font/ttf",
            include_bytes!("../../../assets/pdfjs/standard_fonts/LiberationSans-Bold.ttf"),
        ),
        "/standard_fonts/LiberationSans-BoldItalic.ttf" => (
            "font/ttf",
            include_bytes!("../../../assets/pdfjs/standard_fonts/LiberationSans-BoldItalic.ttf"),
        ),
        "/standard_fonts/LiberationSans-Italic.ttf" => (
            "font/ttf",
            include_bytes!("../../../assets/pdfjs/standard_fonts/LiberationSans-Italic.ttf"),
        ),
        "/standard_fonts/LiberationSans-Regular.ttf" => (
            "font/ttf",
            include_bytes!("../../../assets/pdfjs/standard_fonts/LiberationSans-Regular.ttf"),
        ),
        "/wasm/jbig2.wasm" => (
            "application/wasm",
            include_bytes!("../../../assets/pdfjs/wasm/jbig2.wasm"),
        ),
        "/wasm/jbig2_nowasm_fallback.js" => (
            "text/javascript",
            include_bytes!("../../../assets/pdfjs/wasm/jbig2_nowasm_fallback.js"),
        ),
        "/wasm/openjpeg.wasm" => (
            "application/wasm",
            include_bytes!("../../../assets/pdfjs/wasm/openjpeg.wasm"),
        ),
        "/wasm/openjpeg_nowasm_fallback.js" => (
            "text/javascript",
            include_bytes!("../../../assets/pdfjs/wasm/openjpeg_nowasm_fallback.js"),
        ),
        "/wasm/qcms_bg.wasm" => (
            "application/wasm",
            include_bytes!("../../../assets/pdfjs/wasm/qcms_bg.wasm"),
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::lookup;

    use std::{fs, path::PathBuf};

    #[test]
    fn auxiliary_asset_lookup_is_fail_closed() {
        assert!(lookup("/wasm/openjpeg.wasm").is_some());
        assert!(lookup("/cmaps/Adobe-Japan1-UCS2.bcmap").is_some());
        assert!(lookup("/standard_fonts/FoxitSerif.pfb").is_some());
        assert!(lookup("/icc/CGATS001Compat-v2-micro.icc").is_some());
        for path in [
            "/wasm/../pdf.mjs",
            "/cmaps/%2e%2e/pdf.mjs",
            "/standard_fonts/../../viewer.mjs",
            "/licenses/bcmaps-LICENSE",
            "/fixtures/bug_jpx.pdf",
            "/wasm/not-real.wasm",
        ] {
            assert!(lookup(path).is_none(), "{path} não pode ser servido");
        }
    }

    #[test]
    fn every_vendored_runtime_asset_has_an_exact_route_and_mime() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/pdfjs");
        let mut checked = 0usize;
        for family in ["cmaps", "standard_fonts", "wasm", "icc"] {
            let directory = root.join(family);
            for entry in fs::read_dir(&directory)
                .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
            {
                let path = entry
                    .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
                    .path();
                if !path.is_file() {
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("nome de asset PDF.js em UTF-8");
                let route = format!("/{family}/{name}");
                let (content_type, bytes) = lookup(&route)
                    .unwrap_or_else(|| panic!("asset vendorizado sem rota exata: {route}"));
                assert_eq!(
                    bytes.len() as u64,
                    fs::metadata(&path)
                        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
                        .len(),
                    "a rota deve servir todos os bytes de {route}"
                );
                let expected_type = match path.extension().and_then(|ext| ext.to_str()) {
                    Some("wasm") => "application/wasm",
                    Some("js") => "text/javascript",
                    Some("ttf") => "font/ttf",
                    Some("icc") => "application/vnd.iccprofile",
                    _ => "application/octet-stream",
                };
                assert_eq!(content_type, expected_type, "MIME incorreto para {route}");
                checked += 1;
            }
        }
        assert_eq!(
            checked, 188,
            "a allowlist deve cobrir todos os 188 assets auxiliares de runtime"
        );
    }
}
