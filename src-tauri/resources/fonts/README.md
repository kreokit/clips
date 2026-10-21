# Caption fonts

Free fonts bundled with KreoKit Clips (copied from KreoKit Captions) for caption styles. The editor preview
(`src/styles/caption-fonts.css`) and the burned-in export (libass `fontsdir`, see
`src-tauri/src/export.rs`) both use these exact files, so they render the same font.

| Family | Files | Licence | Source |
|---|---|---|---|
| Inter | Inter-Regular.ttf, Inter-Bold.ttf | SIL OFL 1.1 (OFL-Inter.txt) | github.com/rsms/inter, release 4.1 (static TTF) |
| Montserrat | Montserrat-Regular.ttf, Montserrat-Bold.ttf | SIL OFL 1.1 (OFL-Montserrat.txt) | github.com/JulietaUla/Montserrat |
| Poppins | Poppins-Regular.ttf, Poppins-Bold.ttf | SIL OFL 1.1 (OFL-Poppins.txt) | github.com/google/fonts (ofl/poppins) |
| Bebas Neue | BebasNeue-Regular.ttf | SIL OFL 1.1 (OFL-BebasNeue.txt) | github.com/google/fonts (ofl/bebasneue) |

Static (not variable) TTFs are used so libass picks the exact weight.
