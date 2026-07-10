# PennyPrompt — Auditoría estratégica de avance y diferenciales

**Fecha:** 2026-07-05 · **Revisión 1.1:** 2026-07-06 (añade §8 D7–D9, §10 palancas de adopción/GTM, amplía roadmap B/C con issues `#230`–`#234`, y corrige claims contra prior-art verificado de LiteLLM/MCP)
**Rama auditada:** `feat/m6-issue-202-run-orchestration` (post `v0.1.0-alpha.3`, alpha.4 en curso)
**Autor de la auditoría:** revisión asistida sobre el estado real del código, no sobre el copy de marketing.
**Alcance:** avance del producto, seguridad, escalabilidad, funcionalidad, panorama competitivo, diferenciales actuales y propuestos, y un roadmap accionable para aumentar adopción.

> Nota de método: todas las afirmaciones sobre el estado del producto se contrastaron contra `crates/`, `migrations/`, `prices/`, `presets/` y los docs de release. Donde el README promete algo que el código todavía no cumple, se marca explícitamente como **brecha**, porque una brecha entre promesa y superficie real es exactamente lo que frena la adopción.

---

## 0. TL;DR ejecutivo

PennyPrompt ya es un producto real, no un prototipo: ~19.5k líneas de Rust, 12 crates con fronteras limpias, 190 tests, ledger atómico con `BEGIN IMMEDIATE`, dinero en enteros (micros), tres releases alpha publicados y automatización de release funcionando. **El núcleo financiero es sólido y defendible.**

Pero el producto está posicionado como "guardrail de costos para agentes de IA que funciona con tu agente sin cambiar nada", y hay **dos brechas que atacan directamente esa promesa central**:

1. **No hay ingreso nativo Anthropic (`/v1/messages`).** El proxy solo acepta formato OpenAI (`/v1/chat/completions`). Los agentes tipo OpenClaw/claw-code que el propio README nombra como target primario hablan Messages API nativo. Hoy, apuntar `ANTHROPIC_BASE_URL` al proxy da 404 en la ruta que el agente realmente usa.
2. **No se contabiliza el prompt caching de Anthropic.** Los agentes de código reutilizan contextos enormes con caché; sin leer `cache_read_input_tokens`/`cache_creation_input_tokens`, el costo reportado es sistemáticamente incorrecto justo en el caso de uso estrella.

La tesis de esta auditoría: **el foso (moat) de PennyPrompt no es "otro gateway LLM" — ese espacio está saturado (LiteLLM, Portkey, Helicone, OpenRouter). El foso es ser el único guardrail *local-first, cero-dependencias, consciente de agentes autónomos*, con enforcement atómico *antes* del gasto.** Las dos brechas de arriba, más 4-5 diferenciales nuevos que se detallan abajo, son lo que convierte ese foso en adopción.

---

## 1. El dolor real que resolvemos (máxima abstracción)

Subiendo un nivel de abstracción sobre "controlar costos": el dolor de fondo tiene tres capas.

**Capa 1 — Pérdida de control sobre un gasto que se volvió variable de golpe.**
El evento fundacional (el fin de la tarifa plana para 135k instancias de OpenClaw el 2026-04-04) convirtió un costo *fijo y predecible* en uno *variable, invisible y potencialmente ilimitado*. El agente autónomo es un proceso que gasta dinero real en un bucle, sin un humano mirando cada iteración. Es la primera vez que el desarrollador individual tiene un proceso local que puede quemar $50 mientras almuerza.

**Capa 2 — Asimetría de información temporal.**
El costo se conoce *después* de incurrirlo. Los dashboards de proveedor son agregados y con retraso. El desarrollador no puede responder tres preguntas básicas *en el momento en que importan*:
- *Antes:* "¿cuánto me va a costar esta tarea?" → hoy no tiene respuesta.
- *Durante:* "¿esto se está yendo de las manos ahora mismo?" → hoy se entera por la factura.
- *Después:* "¿en qué se fue exactamente el dinero?" → hoy solo ve un total.

PennyPrompt existe para colapsar esa asimetría: **estimar antes, proteger durante, explicar después.**

**Capa 3 — El agente no es un usuario, es un bucle.**
Esta es la abstracción clave que separa a PennyPrompt de todos los gateways genéricos. Un gateway LLM tradicional modela *aplicaciones con usuarios* (claves virtuales, rate limits por API key, tags por equipo). Un agente autónomo modela *un proceso que reintenta, compacta memoria, y puede entrar en bucles de fallo*. El fallo económico característico de un agente —reintentar la misma tool fallida 30 veces— **no tiene análogo en el mundo de las apps** y por eso los gateways genéricos no lo detectan. PennyPrompt trata el agente como lo que es: un bucle con tarjeta de crédito.

> **El público que siente este dolor con más agudeza:** el desarrollador indie y el equipo pequeño (2-10) que corre agentes de código autónomos localmente, que fue expulsado de la tarifa plana, y para quien levantar un LiteLLM + PostgreSQL + Redis en la nube es desproporcionado. Ese es el wedge de adopción.

---

## 2. Estado del avance (qué está construido y con qué madurez)

| Área | Estado | Evidencia |
|------|--------|-----------|
| Workspace / arquitectura | ✅ Sólido | 12 crates, grafo de dependencias limpio (leaf `penny-types`/`penny-config`), 19.5k LOC |
| Núcleo financiero (ledger) | ✅ Sólido | `reserve/reconcile/release`, `BEGIN IMMEDIATE`, tests de concurrencia |
| Tipo dinero | ✅ Sólido | `Money(i64)` en micros — migraciones 0008/0009 movieron todo a enteros. Sin deriva de float |
| Presupuestos + modos | ✅ Funciona | observe/guard, fail-closed en guard, soft/hard, ventanas day/week/month |
| Detección de bucles | ✅ Funciona | burn-rate, fallos de tool repetidos, similitud de contenido (sha256 de primeros 500 chars) |
| Adaptadores de proveedor | 🟡 Parcial | Anthropic + OpenAI + Mock. Streaming SSE en ambos. Solo 2 proveedores reales |
| Pricebook | 🟡 Parcial | Local versionado; 7 modelos Anthropic + 3 OpenAI. Sin feed remoto firmado |
| Superficie de ingreso proxy | 🔴 Brecha | Solo `/v1/chat/completions`. **Sin `/v1/messages` nativo Anthropic** |
| Prompt caching accounting | 🔴 Brecha | No se leen tokens de caché → costo incorrecto en agentes de código |
| Admin plane | 🟡 Intencional | Reports, budgets, health, SSE de eventos. **Sin auth** (documentado como local-only) |
| CLI | ✅ Rico | init, serve, estimate, run, report, budget, detect, tail, doctor, prices, config, dashboard |
| `serve --daemon` | ✅ Nuevo (alpha.4) | #201 |
| `run <agent>` orquestación | 🟡 Mínimo | #202 — dry-run + `--execute` limitado a agentes que respetan base URL OpenAI-compatible |
| Release / CI | ✅ Maduro | `cargo audit` como gate, checksums, matriz multi-arch, 3 alphas publicados |

**Lectura:** el proyecto ejecutó M1–M6 con disciplina. La deuda no está en el núcleo (que es la parte difícil y está bien hecha) sino en la **superficie de compatibilidad** y en algunos **detalles de exactitud de costo** que, paradójicamente, son los que el usuario *ve primero*.

---

## 3. Panorama competitivo — alternativas en otros repos y dónde está OpenClaw

El error estratégico a evitar es competir en la categoría equivocada. Hay tres categorías y PennyPrompt solo debería pelear en una.

### 3.1 Los gateways/observabilidad (la categoría saturada — NO competir de frente)

| Herramienta | Qué es | Enforcement de presupuesto | Requisitos | Orientación |
|-------------|--------|----------------------------|------------|-------------|
| **LiteLLM** | Proxy Python, 100+ LLMs | `max_budget` por key/user/team, ventanas múltiples | **PostgreSQL + Redis** | Equipos/nube, claves virtuales |
| **Portkey** | LLMOps full-stack; gateway open-source (Apache 2.0, mar-2026) | Presupuestos + guardrails, PII redaction, jailbreak detection | Gateway self-host + plataforma | Producción/enterprise |
| **Helicone** | Observabilidad + proxy ligero, open-source | Tracking + rate limiting | Self-host o SaaS | Logging/analítica. **Adquirido por Mintlify 2026, en modo mantenimiento** |
| **OpenRouter** | Agregador hosted, 300+ modelos | — (marca 5.5% fee) | Ninguno (SaaS) | Simplicidad, una API key |

**Conclusión:** todos estos modelan *aplicaciones con usuarios* y casi todos hacen tracking de gasto *después* de la llamada (o límites blandos). LiteLLM es el más cercano en features de presupuesto, pero su enforcement no es una reserva atómica pre-dispatch a prueba de concurrencia, y su costo de operación (Python + Postgres + Redis) es desproporcionado para un dev individual. **PennyPrompt pierde si intenta ser "LiteLLM pero en Rust". Gana si es la categoría de al lado.**

### 3.2 Los routers (complementarios — componer, no competir)

NadirClaw y similares *eligen el modelo*. PennyPrompt explícitamente **no** es un router (lo dice el README y es correcto). La cadena natural es `Agente → NadirClaw → PennyPrompt → Proveedor`. Esto es un activo: no hay que construir routing, hay que integrarse limpiamente con él.

### 3.3 Dónde está OpenClaw (el hospedero del dolor)

OpenClaw (y claw-code) es el **agente autónomo de código** que vive en la terminal del desarrollador y que sufrió el cambio de tarifa. No es un competidor: **es el sustrato sobre el que PennyPrompt se instala**. La pregunta estratégica no es "¿cómo le gano a OpenClaw?" sino "¿cómo me vuelvo la capa por defecto que todo usuario de OpenClaw instala el día 1?". Eso exige:
- Compatibilidad nativa perfecta con cómo OpenClaw habla (→ brecha `/v1/messages`).
- Exactitud de costo en el patrón real de OpenClaw (→ brecha prompt caching).
- Fricción cero de instalación (single binary — **esto ya lo tenemos y es enorme**).

---

## 4. Nuestros diferenciales ACTUALES (lo que ya nos separa)

Estos son reales y ya están en el código. Hay que protegerlos y hacerlos legibles en el mensaje.

1. **Local-first, single binary, cero dependencias externas.** ~15MB, SQLite embebido, sin PostgreSQL/Redis/Docker. Contra LiteLLM/Portkey esto es un diferencial de *fricción* y de *privacidad* (el tráfico no sale de la máquina salvo hacia el proveedor). Para el dev indie es la diferencia entre "lo instalo en 2 min" y "no lo instalo".

2. **Enforcement atómico *antes* del gasto (reserva en ledger).** RESERVE→DISPATCH→RECONCILE en una transacción SQLite con `BEGIN IMMEDIATE`. La mayoría de competidores contabilizan *después*; PennyPrompt bloquea la request N+1 que rompería el límite, y es correcto bajo concurrencia. Es un diferencial *técnico y verificable*.

3. **Semántica HTTP 402 pensada para agentes, no 429.** El agente reintenta 429; 402 `retryable:false` le dice "para y pregúntale al humano". Es un detalle pequeño con impacto enorme en un bucle autónomo. Ningún gateway genérico piensa en esto.

4. **Detección de bucles de agente (burn-rate, fallos de tool, similitud).** Feature que *no existe* en la categoría gateway porque nace de modelar el agente como bucle. Es el diferencial más difícil de copiar porque requiere pensar en agentes, no en apps.

5. **Auto-atribución sin headers custom** (proyecto por git root, sesión por ventana temporal). Reports útiles desde la primera request, cero config. Los competidores exigen claves virtuales o tags.

6. **Estimación pre-ejecución** ("¿cuánto costará esto?"). Rara en el mercado; responde la pregunta *antes* que casi nadie más responde.

7. **Núcleo financiero correcto por diseño** (dinero en enteros micros, ledger append-only auditable). Confianza: cuando el producto dice "$4.23", es $4.23.

---

## 5. Hallazgos de seguridad

Ordenados por relevancia para adopción/operación real.

| # | Hallazgo | Severidad | Nota |
|---|----------|-----------|------|
| S1 | **Admin plane sin autenticación.** No hay bearer/admin-token (confirmado: cero referencias a auth en `penny-admin`). | Media (mitigada por diseño) | Ya documentado como local-only y loopback/unix-socket. Aceptable para alpha, pero es un **techo de adopción** para el salto a "team". Cualquier proceso local puede leer reports y **mutar budgets** vía `POST /admin/budgets` → efectivamente desactivar el guardrail. |
| S2 | **Sin ruta de reset/gestión de claves de proveedor.** Las API keys se leen de env (`api_key_env`). Bien (no se persisten), pero no hay rotación ni scoping. | Baja | Correcto para alpha; documentar que las keys nunca tocan la DB es un *punto de venta de privacidad*. |
| S3 | **SQL dinámico en reports** (group key / join variant). | Baja (controlada) | Ya auditado: fragmentos vienen de enums, filtros usan bind params. Mantener el guardrail y migrar a query-builder si crece. |
| S4 | **`cargo audit` como gate** ya integrado (rustls-webpki refrescado). | ✅ Positivo | Buena higiene. Mantener el gate en cada release. |
| S5 | **Cleanup de payload / strip ANSI** en el proxy. | ✅ Positivo | Reduce superficie de inyección de secuencias de terminal en outputs que el operador ve en `tail`. |

**Recomendación de seguridad de mayor palanca:** convertir la ausencia de auth de "limitación" en una *decisión de arquitectura con puerta de salida*: mantener local-only por defecto, pero diseñar ya el contrato de token admin (aunque no se implemente) para que "team mode" no requiera rediseño. La mutación de budgets vía admin sin auth es el riesgo más concreto: un agente comprometido que descubra el puerto admin puede subir su propio límite.

---

## 6. Hallazgos de escalabilidad

| # | Hallazgo | Impacto | Recomendación |
|---|----------|---------|---------------|
| E1 | **`max_connections(1)` en el pool SQLite.** Serializa *todas* las operaciones, no solo las escrituras de reserva. | Techo de throughput bajo concurrencia de muchos agentes/sesiones simultáneas. | Correcto para consistencia local single-node. Para escalar lecturas: separar un pool de lectura (WAL permite lectores concurrentes) del único escritor. Medir antes de optimizar. |
| E2 | **Detección de bucles en memoria** (`HashMap<SessionId, SessionWindow>` con `RwLock`). | Estado no persistente: un restart pierde ventanas y sesiones pausadas. | Aceptable para alpha. Documentar que `detect resume` y el estado de pausa no sobreviven restart. Para v1, considerar snapshot ligero. |
| E3 | **Sin backpressure explícito ni límite de conexiones entrantes** en el proxy. | Un agente que abra muchas conexiones puede saturar el único writer. | Añadir límite de concurrencia (tower `ConcurrencyLimit`) y timeouts de upstream configurables. |
| E4 | **Un solo nodo, un solo archivo SQLite.** | Multi-máquina / team compartido no soportado. | Ya es no-objetivo de alpha (correcto). PostgreSQL es el camino v1 para team, pero **no antes** de agotar el mercado single-node. |
| E5 | **Pricebook y reconciliación cargan bien**, pero **el reconcile de streaming depende de estimación** cuando el proveedor no manda usage. | Exactitud de costo degradada en streams sin usage final. | Ligado a la brecha de prompt caching (§7). Priorizar exactitud sobre throughput: es la promesa de marca. |

**Lectura:** la escalabilidad *actual* es la correcta para el público objetivo (single-node local). El riesgo estratégico no es "no escala a 1000 nodos" (no es el mercado) sino **presentar el producto como team-ready antes de tiempo**. Mantener el mensaje honesto: "guardrail local para tu máquina/equipo pequeño".

---

## 7. Hallazgos funcionales / brechas de producto (las que mueven adopción)

Ordenadas por impacto en la promesa central "funciona con tu agente sin cambios".

### F1 — 🔴 Sin ingreso nativo Anthropic (`/v1/messages`) — **la brecha #1**
El router del proxy registra exactamente tres rutas: `/v1/chat/completions`, `/v1/models`, `/internal/health`. No hay `/v1/messages`. El `AnthropicProvider` traduce **salida** al formato Anthropic, pero **no hay superficie de entrada** para un cliente que hable Messages API nativo. Como OpenClaw/claw-code (el target primario declarado) hablan Anthropic nativo, apuntar `ANTHROPIC_BASE_URL=http://localhost:8585/v1` haría que el agente golpee `/v1/messages` → 404. **Esto contradice la tabla de compatibilidad del README.** Es la corrección de mayor ROI de todo el backlog: sin ella, el eslogan de fricción-cero no se cumple para el usuario más importante.

### F2 — 🔴 Sin contabilidad de prompt caching — **la brecha #2**
No se leen `cache_creation_input_tokens` ni `cache_read_input_tokens` (cero referencias en `penny-cost`/`penny-providers`/`penny-types`). Los agentes de código usan caché de prompt de forma agresiva (contexto de repo reutilizado). Un read cacheado cuesta ~10% del input normal y una escritura de caché ~125%; ignorarlos **sobreestima o subestima el costo real de forma material** en exactamente el flujo estrella. La marca es "cuando decimos $X, es $X" — esta brecha la erosiona en silencio.

### F3 — 🟡 Cobertura de proveedores estrecha
Solo Anthropic + OpenAI. Sin Google/Gemini, sin OpenRouter passthrough, sin local (Ollama/vLLM). Muchos devs indie corren modelos locales o mezclan proveedores. Cada proveedor ausente es un segmento que no puede adoptar.

### F4 — 🟡 `run <agent>` todavía mínimo
Dry-run + `--execute` limitado a agentes que respetan base URL OpenAI-compatible. Es la pieza que convertiría a PennyPrompt de "proxy que configuras" a "wrapper que ejecutas" (`pennyprompt run openclaw -- ...`). Alta palanca de UX pero correctamente acotada por ahora.

### F5 — 🟡 Sin dashboard en vivo (solo `tail` textual)
`tail` es funcional pero un TUI/panel es lo que genera el "momento ajá" y las capturas que se comparten (marketing orgánico). Diferido correctamente, pero es un multiplicador de adopción.

### F6 — 🟢 Sin webhooks/alertas salientes
No hay forma de notificar a Slack/Discord/desktop cuando salta un bloqueo o burn-rate. El dev no vive mirando `tail`. Diferido, razonable.

---

## 8. Diferenciales NUEVOS propuestos (para aumentar adopción)

Cada uno se elige por una regla: **profundizar el foso "consciente de agentes + local-first", no diluirlo hacia "otro gateway".**

### D1 — Compatibilidad nativa Anthropic como *feature de portada* (resuelve F1)
No es solo cerrar un bug: convertirlo en mensaje. "Apunta tu OpenClaw a PennyPrompt y funciona idéntico, sin traducciones, sin perder streaming ni tool-use." La compatibilidad *perfecta* con el agente #1 del mercado es en sí un diferencial contra gateways que fuerzan formato OpenAI.

### D2 — "Cost receipt" del agente: exactitud con caché como bandera (resuelve F2)
Ser el **único** guardrail que contabiliza correctamente el prompt caching de Anthropic. Reporte que desglosa: input fresco vs. input cacheado vs. escritura de caché vs. output. Para el usuario de agentes de código esto es *el* número que nadie más le da bien. Es exactitud como diferencial, no como higiene.

### D3 — "Circuit breaker con aprobación humana" (profundiza el diferencial de bucle)
Hoy: bloquear (402) o pausar sesión. Nuevo: cuando una tarea supera un umbral de costo estimado, **pausar y pedir aprobación explícita** (desktop notification / respuesta en CLI) antes de continuar. Convierte el guardrail pasivo en un *human-in-the-loop económico*. Nadie en la categoría gateway hace esto porque nadie modela "tarea de agente" como unidad.

### D4 — Presupuesto por *tarea de agente*, no solo por ventana temporal
Los competidores presupuestan por key/user/día. PennyPrompt puede presupuestar por **tarea** ("no gastes más de $2 resolviendo este issue"), atado a la sesión auto-detectada. Es la unidad mental real del usuario de agentes: "esta feature valió $3". Diferencial conceptual difícil de copiar sin la auto-atribución que ya tenemos.

### D5 — Privacidad/soberanía de datos como diferencial explícito
Contra gateways SaaS (OpenRouter/Portkey managed) y contra Helicone (ahora en mantenimiento): "tu prompt, tu código, tu costo — nada sale de tu máquina salvo la llamada al proveedor." Para healthcare/finance/legal esto es requisito duro. Ya es cierto en el código; falta convertirlo en mensaje de primera línea y quizá una certificación de "no telemetría".

### D6 — Composición explícita con routers (NadirClaw) como estándar
Publicar la integración canónica `Agente → Router → PennyPrompt → Proveedor` con estimación *por modelo candidato*. "El router elige el modelo; PennyPrompt te dice cuánto cuesta cada opción y frena si te pasas." Convierte a un competidor potencial en un canal de distribución.

### D7 — El bucle cost-aware: del guardián al órgano sensorial *(añadido rev. 1.1 — el diferencial estratégico mayor)*
Todo lo anterior trata al agente como algo que hay que *vigilar* (bloquear, pausar, aprobar). El salto conceptual siguiente es darle al agente la señal para **autorregularse**: que sepa cuánto lleva gastado y cuánto le queda, *antes* de chocar contra el muro — y pueda elegir un modelo más barato, compactar contexto o parar solo.

Dos mecanismos, en orden de fricción:
1. **Headers de respuesta** (`X-Penny-Request-Cost-USD`, `X-Penny-Session-Cost-USD`, `X-Penny-Budget-Remaining-USD`, `X-Penny-Budget-Scope`) en cada respuesta proxied — integración cero (issue `#230`).
2. **Servidor MCP de introspección** (`pennyprompt mcp`): el agente pregunta `get_budget_status` / `estimate_cost` al *mismo ledger que hace enforcement* (issue `#232`).

**Prior-art (verificado, no sobre-prometer):** LiteLLM ya expone un header de costo por respuesta (`x-litellm-response-cost`) y headers de rate-limit restante; y existen MCP servers *medidores* de gasto independientes. Lo que **no existe** es el bucle cerrado con autoridad: *presupuesto restante en USD por los scopes que le importan a un agente (tarea/sesión), emitido por el mismo ledger atómico que va a devolver el 402*. El número del header es el número que te bloquea. Medidor pasivo ≠ guardrail introspectable. Combinado con la semántica 402 y el flujo de aprobación (D3), esto convierte a PennyPrompt en la única pieza que cierra el circuito percepción→decisión→enforcement.

### D8 — Prueba de exactitud publicada: el benchmark de paridad de factura *(añadido rev. 1.1)*
La marca es "cuando decimos $X, es $X" — pero un claim sin evidencia reproducible es solo marketing. Un harness que corre una carga de trabajo representativa (stream, tools, caché) contra proveedores reales y publica la desviación entre el costo reportado por PennyPrompt y el facturado por el proveedor (**objetivo: ≤1%**), re-ejecutable por terceros, convierte la exactitud en un *hecho verificable* y además actúa como red de regresión permanente del accounting (issue `#231`). Para un producto de confianza, la prueba **es** el marketing.

### D9 — Visibilidad incrustada en el flujo del desarrollador *(añadido rev. 1.1)*
`pennyprompt statusline`: un segmento de una línea (`$1.42 session · $6.20/hr · 62% day`, <50ms) embebible en la statusline de Claude Code/OpenClaw, starship o tmux (issue `#233`). El valor del producto queda a la vista todo el día **y aparece en cada captura de pantalla que el usuario comparte** — distribución orgánica incrustada en el flujo de trabajo, complementaria al TUI (`#218`).

---

## 9. Roadmap detallado hacia los diferenciales

Principio rector: **primero cerrar las dos brechas que rompen la promesa central (F1, F2), luego profundizar el foso de agente (D3, D4), y solo entonces expandir alcance (proveedores, team).** Expandir antes de cerrar las brechas es construir sobre una promesa incumplida.

### Fase A — "Cumplir la promesa" (alpha.4 → alpha.5) · *bloqueante para adopción*
Objetivo: que la tabla de compatibilidad del README sea literalmente cierta y que el número de costo sea correcto.

- **A1. Ingreso nativo Anthropic `/v1/messages`** (cierra F1 → D1).
  - Nueva ruta en `build_router`. Normalizador Messages→`NormalizedRequest`. Preservar streaming SSE nativo Anthropic (event: message_start/content_block_delta/message_delta/message_stop) y `tool_use`.
  - Test de integración: request Messages nativa → mock → 200 con formato Anthropic, ledger reconciliado.
  - Actualizar README para que la afirmación OpenClaw sea verificable end-to-end.
- **A2. Contabilidad de prompt caching** (cierra F2 → D2).
  - Extender `AccountedUsage` y pricebook con tarifas de `cache_read`/`cache_write`. Leer los campos de usage de Anthropic (y `prompt_tokens_details.cached_tokens` de OpenAI).
  - Reconcile usa las cuatro categorías. Reports desglosan input fresco/cacheado/output.
  - Fixtures de calibración como en el tokenizer dispatch (`#184`).
- **A3. Límite de concurrencia + timeout de upstream** (E3).
  - `tower` ConcurrencyLimit y timeout configurable. Test de saturación.
- **A4. Smoke test del instalador** (#203, ya en alpha.4). Cerrar.

**Salida de fase:** un usuario de OpenClaw instala, apunta, corre una tarea real con caché, y el costo reportado coincide con la factura del proveedor dentro de un margen pequeño. *Ese* es el momento de credibilidad.

### Fase B — "Profundizar el foso de agente" (alpha.5 → beta) · *diferenciación*
Objetivo: features que la categoría gateway estructuralmente no tiene.

- **B1. Presupuesto por tarea/sesión** (D4). Nuevo `ScopeType::Task` atado a sesión auto-detectada; CLI `budget set task:<id>`; estimación consume presupuesto de tarea.
- **B2. Circuit breaker con aprobación humana** (D3). Nueva acción `require_approval` además de `alert`/`pause`. Notificación desktop + resume vía CLI. Evento `ApprovalRequested`.
- **B3. `pennyprompt run <agent>` real** (F4). `run openclaw -- <args>` levanta proxy efímero, inyecta base URL, adjunta atribución de tarea, tears down al terminar. Convierte el proxy en wrapper.
- **B4. Webhooks/alertas salientes** (F6). Slack/Discord/desktop en bloqueo, burn-rate, aprobación. Config `[detect.webhooks]`.
- **B5. Headers de cost-feedback** (D7, `#230`) *(rev. 1.1)*. Costo del request + presupuesto restante por scope en cada respuesta, emitidos por el ledger que hace enforcement. Barato de construir, abre el bucle cost-aware.
- **B6. Benchmark de paridad de factura** (D8, `#231`) *(rev. 1.1)*. Solo tiene sentido después de A1+A2; secuenciarlo al final del tren. Publica la evidencia del criterio de salida de la Fase A.

**Salida de fase:** PennyPrompt hace cosas que LiteLLM/Portkey no pueden hacer *por diseño*, no por falta de features — y la exactitud deja de ser un claim para ser un reporte reproducible.

### Fase C — "Expandir alcance sin diluir" (beta → v1) · *crecimiento*
- **C1. Proveedores** (F3): Gemini/Google, passthrough OpenRouter, local (Ollama/vLLM). Cada uno abre un segmento.
- **C2. TUI/dashboard en vivo** (F5). El multiplicador de marketing orgánico (capturas compartibles).
- **C3. Feed de pricebook remoto firmado.** Mantener exactitud sin releases manuales; firma para no romper el modelo "sin scraping, sin llamadas externas no verificadas".
- **C4. Diferencial de privacidad explícito** (D5): auditoría "cero telemetría", doc de soberanía de datos, quizá attestation.
- **C5. Integración canónica con router** (D6): recetas NadirClaw, estimación multi-modelo.
- **C6. Servidor MCP de introspección de presupuesto** (D7, `#232`) *(rev. 1.1)*. Cierra el bucle cost-aware: read-only, ≤5 tools, respaldado por el ledger de enforcement.
- **C7. Statusline embebible** (D9, `#233`) *(rev. 1.1)*. <50ms, degradación elegante, recetas para Claude Code/OpenClaw/starship/tmux.
- **C8. Canales de distribución** (`#234`) *(rev. 1.1)*. Homebrew tap, `cargo-binstall`, guías de integración de una página por agente. El multiplicador más barato de todo lo demás (ver §10).

### Fase D — "Team sin traicionar local-first" (v1+) · *solo si el mercado single-node se agota*
- **D-1. Auth del admin plane** (S1): diseñar el contrato de token *ahora* (Fase A, sin implementar) para no rediseñar aquí.
- **D-2. Backend PostgreSQL opcional** (E4) detrás del mismo trait de store. SQLite sigue siendo el default.
- **D-3. Pool de lectura separado del writer** (E1).

**Regla de oro para la Fase D:** no empezar hasta tener evidencia de demanda de equipos. El riesgo de muerte no es "no tenemos team mode", es "diluimos el foso local-first persiguiendo enterprise antes de dominar el nicho".

---

## 10. Palancas de adopción (go-to-market) — *añadido rev. 1.1*

La revisión crítica de la v1.0 de este documento reveló su vacío mayor: era 100% producto. Pero para un proyecto open-source, **adopción = producto × distribución × confianza × visibilidad** — y tres de los cuatro factores no aparecían. Las fases A–D construyen el producto; esta sección construye el resto. Sin esto, un buen producto se queda en 30 estrellas.

### 10.1 Confianza (la prueba es el marketing)
Para un *guardrail de dinero*, la confianza no es un nice-to-have: es la única razón de compra. Palancas:
- **Benchmark de paridad de factura publicado por release** (`#231`, D8). "≤1% de desviación, re-ejecútalo tú mismo" vale más que cualquier post.
- **Cero telemetría verificable** (`#220`, D5): no es solo privacidad, es coherencia — un producto que vigila tu gasto no puede vigilarte a ti.
- **Honesty ledger** en el backlog (ya existe): las brechas se publican con fecha e issue, no se esconden. Mantenerlo es una política, no un documento.
- `cargo audit` como gate público (ya existe).

### 10.2 Distribución (canales, `#234`)
- **Homebrew tap + `cargo-binstall`**: el single binary es el diferencial de fricción; sin `brew install` está desaprovechado en el segmento macOS que domina el público objetivo.
- **Guías de integración de una página por agente** (OpenClaw, claw-code, Cursor, Codex, Continue): el paste exacto + un paso de verificación. Cada guía es además una landing indexable ("openclaw cost limit", "cursor budget cap") — SEO orgánico de intención altísima.
- **Listas y registries**: awesome-rust, awesome-llm; registries MCP cuando `#232` exista (cada registry es un canal).

### 10.3 Visibilidad diaria (el producto se muestra solo)
- **Statusline** (`#233`, D9) y **TUI** (`#218`): el costo en vivo aparece en la pantalla del dev todo el día y en cada captura que comparte. Marketing incrustado en el flujo de trabajo, coherente con cero telemetría: no rastreamos usuarios, los usuarios nos muestran.
- **Headers** (`#230`): el prefijo `X-Penny-*` viaja en logs y debug de terceros — el nombre se propaga solo.

### 10.4 Comunidad (convertir el patrón adapter en cantera)
- Los **adaptadores de proveedor** (C1–C3) son el `good-first-issue` perfecto: patrón repetible, bien delimitado, con dos ejemplos de referencia en el árbol. Etiquetar y documentar "cómo añadir un proveedor" convierte la brecha F3 en cantera de contribuidores en vez de backlog propio.
- Cada guía por agente (`#234`) termina con "¿tu agente no está? PR bienvenido" — el directorio de integraciones crece solo.

### 10.5 Métricas norte sin telemetría
Coherencia con D5: no se instrumenta al usuario. Se mide con señales públicas:
- **Descargas de release por versión** (`gh api`), estrellas/semana, analytics públicas del tap de Homebrew.
- **Issues/discusiones abiertos por terceros** — la señal de adopción real más fuerte que existe para un proyecto local-first.
- North star sugerida: **descargas semanales de release** + **issues de no-mantenedores/mes**. Ritual: snapshot mensual en `docs/status-*.md`.

### 10.6 Secuencia de lanzamiento ligada a los trenes de release
La regla: **un solo gran disparo, y solo cuando la promesa sea demostrable.** Lanzar antes de la Fase A quemaría el único tiro de credibilidad (la brecha F1 sería el primer comentario del hilo).
- **alpha.5** — sin promoción: es un release de corrección. Solo actualizar guías/README.
- **alpha.6** — primer contenido técnico: "cómo funciona la reserva atómica de presupuesto", "invoice-parity report #1". Construye autoridad, no tráfico.
- **beta.1** — **el lanzamiento** (Show HN, r/LocalLLaMA, lobste.rs): con paridad publicada, demo del bucle cost-aware (GIF statusline + TUI + un 402 salvando dinero), `brew install` funcionando y cinco guías por agente. Todo alineado en un solo momento.
- **v1.0.0** — anuncio de estabilidad; homebrew-core, winget/apt (los registries "serios" exigen no-prerelease).

### 10.7 Sostenibilidad (nota de una línea)
GitHub Sponsors desde ya (fricción cero). Si algún día hay monetización, vive en el team-tier (v1+), **nunca** como gate de features locales: local-first gratis *es* el moat, no el cebo de un freemium.

---

## 11. Prioridad recomendada (si solo se hace una cosa por trimestre)

1. **A1 + A2** (ingreso nativo Anthropic + prompt caching). Sin esto, la promesa central no se cumple para el usuario #1. Todo lo demás es secundario.
2. **B5 + B2 + B4** (headers cost-feedback + circuit breaker con aprobación + alertas). El bucle agente completo: percepción, decisión, enforcement. Es el diferencial más puro de "consciente de agentes" y el más difícil de copiar.
3. **B6 + C8** (benchmark de paridad + canales de distribución). Confianza demostrable + fricción de instalación mínima = las precondiciones del lanzamiento de beta.1 (§10.6).
4. **B3** (`run` real) + **C7/C2** (statusline + dashboard). UX y visibilidad orgánica.
5. **C1 proveedores + C6 MCP** para abrir segmentos y cerrar el bucle cost-aware, en el orden de demanda observada.

---

## 12. Riesgos estratégicos y anti-objetivos

- **Riesgo #1 — Competir como gateway genérico.** Si el roadmap deriva hacia "features de LiteLLM en Rust", se pierde. El foso es agente + local-first, no cobertura de 100 modelos.
- **Riesgo #2 — Prometer team/enterprise antes de dominar el nicho.** Diluye el mensaje y el diseño. Mantener honestidad de alcance (ya se hace bien en los docs).
- **Riesgo #3 — Exactitud silenciosamente incorrecta** (F2). Un guardrail de costos que reporta mal el costo pierde su única razón de existir. La exactitud es la marca, no una feature.
- **Riesgo #4 — Brecha promesa/realidad** (F1). El README promete compatibilidad que el router no cumple. Cerrar la brecha o ajustar la promesa; no dejar ninguna abierta.

**Anti-objetivos que hay que mantener** (ya bien definidos en el backlog): no ser router, no ser gateway enterprise, no ser SaaS, no scrapear precios, no exponer admin sin auth fuera de loopback.

---

## 13. Síntesis en una frase

> **PennyPrompt no es "un gateway LLM más": es el primer guardrail de costos que trata al agente autónomo como lo que es —un bucle local con tarjeta de crédito— y lo hace en un binario de 15MB sin dependencias. El moat ya existe en el código. La adopción depende de tres movimientos en orden: (1) cerrar las dos brechas que rompen la promesa central (ingreso nativo Anthropic y contabilidad de caché), (2) construir lo que ningún gateway genérico puede copiar —presupuesto por tarea, circuit-breaker con aprobación humana y el bucle cost-aware donde el mismo ledger que bloquea es el que informa al agente—, y (3) lanzar una sola vez, con la exactitud demostrada por un benchmark de paridad reproducible y la instalación a un comando de distancia.**

---

### Anexo — Evidencia de código consultada

- Rutas del proxy: `crates/penny-proxy/src/lib.rs` (`build_router`, líneas ~281-285) — solo 3 rutas, sin `/v1/messages` de ingreso.
- Adaptadores: `crates/penny-providers/src/lib.rs` — Anthropic/OpenAI/Mock; Anthropic traduce salida a `/v1/messages` (~272).
- Prompt caching: sin referencias a `cache_read`/`cache_creation` en `penny-cost`/`penny-providers`/`penny-types`.
- Pool SQLite: `crates/penny-store/src/lib.rs:106-114` — `max_connections(1)`, WAL, foreign_keys.
- Ledger atómico: `crates/penny-ledger/src/lib.rs:373-375` — `begin_with("BEGIN IMMEDIATE")`.
- Dinero: `crates/penny-types/src/lib.rs` — `Money(i64)` en micros.
- Auth admin: sin referencias a bearer/token/auth en `crates/penny-admin/`.
- Pricebook: `prices/anthropic.toml` (7 modelos), `prices/openai.toml` (3 modelos); sin Gemini/local.
- Tests: 190 (`#[test]`/`#[tokio::test]`/`#[sqlx::test]`).

### Anexo — Fuentes competitivas

- [LiteLLM — Budgets & Rate Limits](https://docs.litellm.ai/docs/proxy/users) · [Virtual Keys](https://docs.litellm.ai/docs/proxy/virtual_keys) · [Spend Tracking](https://docs.litellm.ai/docs/proxy/cost_tracking)
- [LLM Gateway 2026: OpenRouter vs LiteLLM vs Portkey vs Helicone](https://klymentiev.com/blog/llm-gateway-guide)
- [Best LLM Gateways 2026 — Braintrust](https://www.braintrust.dev/articles/best-llm-gateways-2026)
- [7 Best OpenRouter Alternatives 2026](https://ofox.ai/blog/openrouter-alternatives-2026/)

Prior-art verificado para D7 (rev. 1.1):
- [LiteLLM — Response Headers](https://docs.litellm.ai/docs/proxy/response_headers) (`x-litellm-response-cost`, rate-limit remaining) — costo por respuesta existe; presupuesto-restante-USD por tarea/sesión desde el ledger de enforcement, no.
- [LLM Usage & Cost Tracker (MCP, Glama)](https://glama.ai/mcp/servers/zhaoyue722/llm-usage-mcp) y [Agent Budget Guard](https://earezki.com/ai-news/2026-03-02-i-built-an-mcp-server-so-my-ai-agent-can-track-its-own-spending/) — medidores MCP pasivos existen; introspección respaldada por el guardrail que bloquea, no.
