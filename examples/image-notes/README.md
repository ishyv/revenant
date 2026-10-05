# Luma — Image Notes

Una utilidad local para añadir varias carpetas de imágenes, escribir notas y
encontrar imágenes buscando esas notas. Es una aplicación de prueba construida
con Revenant 0.3, Svelte y capacidades Rust propias.

## Probar en Windows

Con Rust/Cargo, Node/npm, MSVC C++ Build Tools y WebView2 instalados:

```powershell
git clone --branch feat/image-notes-demo https://github.com/ishyv/revenant.git revenant-luma
cd revenant-luma
cargo install --path . --locked
revenant setup
cd examples/image-notes
revenant dev --verbose
```

Si ya tienes el repositorio, puedes crear un checkout independiente:

```powershell
git fetch origin feat/image-notes-demo
git worktree add ../revenant-luma origin/feat/image-notes-demo
cd ../revenant-luma
cargo install --path . --locked
cd examples/image-notes
revenant dev --verbose
```

El primer arranque instala las dependencias, compila el host de escritorio y
genera los tipos desde el contrato Rust. Los siguientes arranques reutilizan el
catálogo y la caché. El frontend necesita el host Tauri: abrir solo Vite en un
navegador no proporciona las capacidades nativas.

La rama incluye el workflow **Luma Windows app**. Una ejecución correcta publica
el artifact **luma-windows-installer**, con un instalador `.exe`, durante 14 días.
Puedes descargarlo desde la ejecución en GitHub Actions y probar sin instalar
Rust ni Node. La existencia del workflow no significa que la compilación ya haya
terminado; comprueba su resultado antes de descargarlo.

## Qué probar

1. Pulsa **Añadir carpeta** y elige una carpeta con JPEG, PNG, WebP, GIF o BMP.
   Se incluyen sus subcarpetas y puedes cancelar el escaneo.
2. Abre una imagen. Escribe una nota y pulsa **Guardar nota** o `Ctrl/⌘ S`.
   También se guarda al salir del campo o cambiar la imagen seleccionada.
3. Busca una palabra de la nota. La búsqueda incluye nombres, rutas relativas
   y anotaciones de todas las carpetas; puedes elegir una raíz en la barra lateral.
4. Añade otra carpeta y repite. **Con anotaciones** filtra los resultados anotados.
5. Cierra y vuelve a abrir: las carpetas, las notas y el índice permanecen.
6. Añade o elimina imágenes en una carpeta y pulsa **Actualizar**. Las notas de
   archivos que conservan su ruta permanecen. Las imágenes desaparecidas dejan
   de mostrarse; sus registros se retienen por si reaparecen.

La búsqueda combina prefijos de palabras: `atardec malag` encuentra una nota
como «Atardecer en Málaga». Ignora diferencias de mayúsculas y diacríticos.
No es una búsqueda de subcadenas arbitrarias ni una búsqueda semántica.

## Datos y comportamiento

- Todo se procesa localmente. Las imágenes originales no se modifican.
- SQLite guarda el catálogo, las notas y el índice FTS5. Las imágenes tienen
  identidades persistentes propias; no se guardan handles de Revenant.
- En Windows los datos están en `%LOCALAPPDATA%\app.revenant.image-notes`.
  En Linux/macOS se usa el directorio local de datos que devuelve `dirs`.
  La ruta también aparece al pasar el cursor por el pie de la barra lateral.
- Las miniaturas se crean al entrar cerca del viewport. Se admiten como máximo
  dos renderizaciones a la vez; el visor tiene prioridad sobre tarjetas en espera.
- Las respuestas contienen JPEGs acotados. Las vistas son de hasta 1440 px y las
  miniaturas de hasta 320 px, con reducción adicional si hace falta limitar bytes.
  Se aplica orientación EXIF y se compone transparencia sobre fondo blanco.
- El catálogo devuelve páginas de 48 imágenes, con máximo nativo de 60.
- **Quitar de la biblioteca** pide confirmación y elimina también las notas de
  esa raíz del catálogo. No elimina los archivos originales.

## Límites de esta primera versión

Es un prototipo funcional para evaluar ergonomía, no una versión con promesa de
rendimiento instantáneo en cualquier colección. Las páginas acotan la interfaz;
no hay una galería infinita. No hay watcher automático: usa **Actualizar**.
Los renombres y movimientos no trasladan automáticamente las notas; la identidad
se mantiene por raíz y ruta relativa. No hay migrador de raíces movidas.

GIF muestra una imagen estática. HEIC, TIFF, SVG, RAW y vídeo no están soportados.
No se siguen enlaces simbólicos ni junctions. Las notas tienen un máximo de
16 KiB de UTF-8; se muestra el fallo si un guardado no se puede completar.
Guarda la nota antes de cerrar; no dependas de un aviso del webview al salir.

El decodificador limita dimensiones y memoria. Una foto demasiado grande,
corrupta o inaccesible muestra un error. La caché en disco no tiene todavía una
política de limpieza por tamaño. No elimines `catalog.sqlite3` para limpiar
miniaturas: con la app cerrada puedes borrar únicamente `renditions/`.

## Qué pertenece a Revenant y qué pertenece a la aplicación

Revenant proporciona el selector de carpeta, los scopes, las operaciones tipadas,
los trabajos/cancelación, la generación de contratos y el empaquetado Tauri.
Luma añade SQLite/FTS5, identidad estable, escaneo/reconciliación, notas, caché de
rendiciones y componentes de galería/inspector. La librería base no se modifica.

`native/src/catalog.rs` contiene esas capacidades. `web/src/lib/types.ts` infiere
sus DTOs desde la fachada compilada. `image-queue.ts` limita admisión y memoria de
URLs. Las operaciones usan workers bloqueantes para escaneo y decodificación.

## Validación

Se verificaron el contrato exportado desde la aplicación Rust real, la generación
de la fachada mediante `BuildStage`, el typecheck de Svelte y el build estático.
Las pruebas nativas cubren reapertura, actualización conservando notas, búsqueda
con acentos, actualización del índice, paginación, notas demasiado grandes y
renderización/cache de un PNG real mayor de 4 MiB. Las pruebas de la cola cubren
concurrencia, prioridad del visor, cancelación de tarjetas y reintentos.

La instalación y la interacción dentro de Windows requieren aceptación en una
máquina Windows; el workflow comprueba compilación y empaquetado, no sustituye
esa prueba manual.

Después de que Revenant haya materializado el SDK del ejemplo:

```powershell
cargo test --manifest-path native/Cargo.toml --locked
cargo fmt --manifest-path native/Cargo.toml -- --check
npm run check --prefix web
npm test --prefix web
revenant build --verbose
```
