# Статус реализации SGF

## Дополнение от 2026-11-14

## Главная цель и текущий исследовательский этап

Главная цель SEMANTICGRAVITIFIELD — получение реальной экономической прибыли. Первая и текущая область реализации — исследование торговых возможностей. Канонические формулировки находятся в PURPOSE.md и ARCHITECTURE.md; приоритеты — в ROADMAP.md.

Ветка `implementation/market-research-core`, PR #15:
- Добавлен `src/research.rs` с детерминированным анализом одного упорядоченного окна Quote/Trade одного инструмента.
- Рассчитываются движение цены между первой и последней сделками, максимальный наблюдавшийся спред, количество событий/сделок/котировок и объём.
- Кандидат формируется только при выполнении явных порогов движения, спреда и числа сделок.
- Кандидат не является торговым сигналом и не доказывает прибыльность. Модель комиссий, проскальзывания, ликвидности, независимой выборки и исполнения ещё не реализована.
- Добавлены позитивные и негативные unit-тесты.
- PR #15 открыт, код не входит в main. CI run #445 на HEAD `ecf3c756315ef0c087aad8364ec3a9d7a17c3e4a` остановился на rustfmt; форматирование исправлено в последующих commits. CI run #448 на HEAD `f168112ddf37fe437a67e859f417550a0fd4db5d` выявил только замечание rustfmt; исправлено. CI run #450 на точном HEAD `76aba71eeb0fbc0966c676bda163659051111957` завершился SUCCESS: rustfmt PASS, Clippy PASS, 52 unit tests PASS и 1 integration test PASS.
- Локальные Rust-проверки в этой рабочей среде не выполнялись: Rust toolchain недоступен.


## Исторический срез на 2026-11-11

Этот раздел имеет приоритет над историческими записями ниже. Проверки и review относятся только к указанному SHA.

### PR #14 — файловое хранилище
- HEAD: `621e670a6dcbda6f0ed11126b99fbf688300d417`; PR OPEN, не слит.
- CI run #431 на точном HEAD `621e670a6dcbda6f0ed11126b99fbf688300d417` завершился SUCCESS: Linux format/Clippy/tests PASS (70 unit + 1 integration), cargo-audit 0.22.2, RustSec и source/license gates PASS; macOS format/Clippy/tests PASS (71 unit + 1 integration).
- Code Review receipt на старом SHA `bd35db9` не переносится на текущий HEAD. Повторные Code Review и отдельный security-focused review на `621e670` запрошены; receipts ещё не получены.
- Исправлены P2 по lock inode, staging directory permissions и pre-existing writable store: lock открывается no-follow/nonblocking и блокируется через `FileLock::from_file`; проверяются owner и nlink; root/generations/staging имеют 0700, новые lock/CURRENT/artifacts — 0600.
- До чтения содержимого отклоняются существующие root/generations с group/world write; parent chain проверяется на владельца root/effective UID и отсутствие writable non-sticky компонентов. Sticky system directories допускаются. Добавлены тесты на writable root, writable generations, writable parent и сохранение исходного содержимого при отказе.
- Filesystem root отклоняется; компоненты `..` в storage path отклоняются, чтобы не менять семантику разрешения пути при symlink. Компоненты `.` нормализуются без разрешения произвольных symlink.
- Границы: доверенная родительская цепочка обязательна; same-UID и privileged attackers не покрываются; полная descriptor-relative реализация и GC/retention не реализованы.
- PR не сливать до успешного CI на текущем SHA, повторного Code Review, отдельного security-focused review и проверки всех review threads.

### PR #13 — синтетическая модель наблюдения
- HEAD: `e88df5693a5cc2cc8fcdcd94ac8d29d78e2fec37`; PR OPEN, не слит; base — PR #12.
- CI run #395 на точном HEAD завершился SUCCESS: rustfmt, Clippy и 79 тестов PASS (48 unit + 1 integration + 14 experiment contract + 16 observer integration).
- P2 по неограниченным дополнительным allocations исправлен: ObservationLimits и observe_with_limits проверяют count/input-byte budgets до BTreeSet, output count/byte budget — до выделения Vec; индексы строятся один раз с одновременной проверкой уникальности. Значения по умолчанию конечные, но настраиваемые; они не являются пределами формата или модели.
- Повторный Code Review и отдельный security-focused review на точном HEAD `e88df56` запрошены; receipts ещё не получены.

### PR #12 — типизированная модель наблюдателя
- HEAD: `886704f6971835809b00bb4065871c29fff5f661`; PR OPEN, не слит.
- CI run #323 на точном HEAD SUCCESS; повторный Code Review receipt на точный SHA получен без существенных замечаний.
- Отдельный security-focused review запрошен, подтверждённого результата пока нет.
- Digest полных определений обеспечивает content binding, но не аутентификацию. Внешний trust anchor остаётся отдельной задачей.

Ни один из PR #12, #13, #14 не включён в main. Открытые PR не считаются реализованными в main.

## Текущая версия

Прототип: 0.1.0  
Язык ядра: Rust  
Репозиторий: WSOBIRW/SEMANTICGRAVITIFIELD  
Последняя проверенная базовая версия main на момент переноса файлового backend: commit `3a8dbdc540d9bccad8b10156475ea536ea7adf08`. Открытые PR ниже не входят в main. Все проверки привязаны к конкретному HEAD и CI run.  
Назначение реализованного ядра: проверка входных рыночных событий, предотвращение повторов и некорректного порядка, snapshot/journal/replay и контракты долговечного восстановления.

Этот документ фиксирует фактическое состояние на момент коммита. Любой CI/review результат относится только к явно указанному SHA.

## Архитектурная документация в main

- ARCHITECTURE.md — назначение, контуры, инварианты и границы доверия.
- ARCHITECTURAL_CONSTRAINTS_REGISTER.md — постоянные инварианты, временные ограничения MVP, ограничения backend и правила эволюции.
- EVIDENCE_MODEL.md — доказательства, качество и независимость источников.
- DATA_PROVENANCE.md — происхождение данных и преобразования.
- UNCERTAINTY_MODEL.md — неполнота, неопределённость и воздержание.
- OBSERVER_MODEL.md — нормативная модель наблюдателя, границ наблюдаемости и влияния процедуры; документ не означает реализацию в коде.
- CONTROLLED_OBSERVER_EXPERIMENT.md — протокол воспроизводимого синтетического эксперимента; минимальное ядро находится в открытом PR #13 и не включено в main, полноценный runner не реализован.
- VERIFICATION_CONTRACT.md — независимая проверка и отрицательные контроли.
- SEMANTIC_INTELLIGENCE.md — исследование структур и гипотез.
- DECISION_CONTRACT.md — результаты и защитные вердикты.
- INTERFACE_CONTRACT.md — внешний контракт.
- IDENTITY_AND_TIME_MODEL.md — целевая модель идентичности и времени; реализация ещё не включена в ядро.
- EVENT_SCHEMA_EVOLUTION.md — контракт эволюции событий, opaque preservation и миграции; реализация ещё не включена в ядро.
- ROADMAP.md — этапы развития.
- AUDIT_FINDINGS.md — findings раннего аудита.
- SNAPSHOT_FORMAT.md, JOURNAL_FORMAT.md, RECOVERY_MANIFEST.md — форматы восстановления.
- FILE_STORAGE_BACKEND.md — контракт первого файлового backend.

Наличие нормативного документа не означает реализацию описанной подсистемы.

## Реализовано и включено в main

- Типизированное рыночное событие с schema_version.
- Quote и Trade с целочисленным представлением цены/объёма.
- Временные метки события и получения в Unix milliseconds UTC.
- Валидация идентификаторов, временных значений, цены, масштаба и crossed quote.
- ClockSkewSuspected при event_time позже received_time без произвольной коррекции часов.
- Дедупликация event_id: неограниченный режим и конфигурируемая FIFO-ёмкость.
- Контроль sequence отдельно по source_id/instrument_id; разрывы фиксируются предупреждением, невозрастающая sequence отклоняется.
- Явный reset_stream, сбрасывающий только sequence watermark.
- Версионированный snapshot SGFS v1 с SHA-256.
- Журнал SGFJ v1 с hash chain, offsets/checkpoints и детерминированным replay.
- Recovery Manifest SGFM v1 и RecoveryPackage.
- Backend-neutral RecoveryStorage contract.
- CAS контракт поколений: expected_current_generation, строгий successor и типизированные ошибки (PR #10, слит в main).

## В разработке, не включено в main

### PR #11 — File Generation Recovery Storage

Ссылка: https://github.com/WSOBIRW/SEMANTICGRAVITIFIELD/pull/11

- Состояние: CLOSED, не слит; закрыт как superseded PR #14.
- Причина: исходная ветка разошлась с main. Реализация перенесена в чистую ветку от актуального main, дополнена read budgets, Cargo.lock и CI с --locked.
- Не использовать PR #11 как актуальный кандидат и не сливать его отдельно.
- Актуальное продолжение: PR #14.

### PR #14 — Recovery Storage с ограниченными чтениями

Ссылка: https://github.com/WSOBIRW/SEMANTICGRAVITIFIELD/pull/14

- Состояние: OPEN, не слит.
- Ветка: `feat/file-generation-recovery-storage-current`, создана от актуального main, чтобы избежать diverged history исходной ветки PR #11.
- Текущий HEAD: `bd35db9e5c80f0b84d3fc06030bd5fc414881f8e`.
- Реализация файлового backend перенесена в чистую ветку. Включены Cargo.toml, Cargo.lock, src/lib.rs, src/file_storage.rs и обновление CI workflow.
- Cargo.lock сгенерирован Cargo в успешном run #297; 21 пакет, включая платформенные зависимости Windows, с версиями и checksum. В CI Clippy и тесты теперь используют --locked. Временный экспорт lockfile как artifact удалён.
- Run #297 на кодовом HEAD `8e734ff`: rustfmt PASS, Clippy PASS, тесты PASS; Cargo.lock artifact получен и добавлен в репозиторий.
- CI run #314 на предыдущем HEAD `cf290ca`: rustfmt PASS, Clippy PASS, 58 unit tests + 1 integration test PASS (59 всего). Cargo.lock и `--locked` PASS. Source/license gate проверил 21 транзитивный пакет: все источники crates.io; лицензии соответствуют allowlist MIT/Apache-2.0/BSD-3-Clause, включая точную нормализацию legacy `MIT/Apache-2.0`.
- По P2 finding code review о пропущенной проверке RustSec advisory database добавлен отдельный gate: установка `cargo-audit` версии `0.22.2` с `--locked` и `cargo audit --file Cargo.lock`. CI run #321 на точном HEAD `82ed2644a76b6cb0393cb8d051b8ab0f444589da`: SUCCESS. rustfmt, Clippy, тесты, установка cargo-audit 0.22.2, RustSec advisory audit и source/license gate прошли. Проверка лицензий/источников охватила 21 транзитивный пакет.
- P2 finding по отсутствию advisory scan исправлен; ответ опубликован, inline thread разрешён после успешного CI run #321. Повторный Code Review на HEAD `82ed264` подтвердил этот commit.
- P2 finding по stale `.staging-*` и `.CURRENT-*` после crash исправлен в `290afc3`: резервирование повторяет попытку с увеличенным token при `AlreadyExists`; чужие пути не изменяются и не удаляются. Добавлены тесты на оба вида stale collision. Inline thread получил ответ и разрешён после проверки исправления.
- Проверка итераций патча: run #337 остановился на rustfmt; run #339 выявил регрессию неполного редактирования (утрачены вспомогательные функции и trait impl); run #343 подтвердил восстановление helpers, но остановился на rustfmt; run #345 выявил отсутствующий trait impl. Эти ошибки исправлены восстановлением `impl RecoveryStorage` из проверенного исходного HEAD. CI run #347 на HEAD `290afc3` завершился SUCCESS: rustfmt, Clippy, тесты, установка cargo-audit 0.22.2, RustSec advisory audit и source/license gates прошли. Повторный Code Review на `290afc3` выявил P2 по macOS: системные пути `/var`, `/tmp`, `/etc` могут быть совместимыми symlink-псевдонимами. Исправление внесено в `156aea8`: нормализуются только фиксированные системные псевдонимы, произвольные пользовательские symlink-ancestor по-прежнему отклоняются. В `09cc3d4` добавлена отдельная macOS CI job. Run #353 на `09cc3d4` завершился SUCCESS: Linux и macOS форматирование, Clippy и тесты, cargo-audit 0.22.2, RustSec advisory и source/license gates прошли. Повторный Code Review на `09cc3d4` выявил три новых P2: относительный root зависел от будущего cwd; FIFO мог блокировать File::open; symlink/FIFO replacement race не закрывался. Исправления внесены в `a72bbea`/`b189f17`/`be729de`: root закрепляется абсолютным путём при open; Unix artifact open использует предварительную symlink_metadata-проверку и `O_NOFOLLOW | O_NONBLOCK`, затем проверяет тип открытого дескриптора; libc 0.2.191 добавлен как прямая зафиксированная зависимость. Тест изменения cwd не меняет глобальный cwd тестового процесса. Run #361 на `1228d01`, #369 на `9d3ee70` и #371 на `8bb0b05` остановились только на rustfmt; форматирование исправлено последовательно в следующих commits. Дополнительно no-follow/nonblocking helper распространён на CURRENT pointer, добавлен тест прямого nonblocking FIFO open. Текущий HEAD `bd35db9`. Run #373: Linux format/Clippy/tests PASS; macOS format/Clippy PASS, тесты выполняются; Linux cargo-audit устанавливается, RustSec и source/license gates ожидают. Повторный Code Review и отдельный security-focused review на `bd35db9` запрошены, receipts не подтверждены.
- Реализованы конфигурируемые per-artifact и aggregate бюджеты чтения. Размеры всех файлов и исходная сумма проверяются до выделения буферов; aggregate budget повторно контролируется по фактически прочитанным байтам между последовательными чтениями, чтобы рост нескольких файлов после metadata check не обходил совокупный предел.
- Добавлены тесты на per-file budget, aggregate budget, рост файла, bounded reader при росте после metadata check и нулевой бюджет.
- Code Review receipts подтверждают просмотр `290afc3` и `09cc3d4`; второй выявил три P2, исправления внесены в `bd35db9`. Новый Code Review и security-focused review на точном HEAD `bd35db9` запрошены, receipts пока не подтверждены.
- Отдельный Security Review не подтверждён. Проверка cargo-audit выявляет известные опубликованные advisories, но не устанавливает личность издателя, доброкачественность кода или отсутствие неизвестных уязвимостей.
- Локальный Rust toolchain в рабочей среде отсутствует; проверки выполняются GitHub Actions.
- Dependency gates разделены: Cargo.lock закрепляет версии и checksums; CI проверяет разрешённые registry sources и license expressions; cargo-audit проверяет известные RustSec advisories. Ни один из этих механизмов не подтверждает личность издателя, доброкачественность кода или отсутствие неизвестных уязвимостей.
- PR #14 не слит.

### PR #13 — Deterministic Synthetic Observer Experiment Core

Ссылка: https://github.com/WSOBIRW/SEMANTICGRAVITIFIELD/pull/13

- Состояние: OPEN, не слит.
- Ветка: `feat/controlled-observer-experiment`.
- Текущий HEAD: `05a3248eac1e3ccb8cdbd81bd72950a5f1eeffbe`.
- Базовая ветка PR: `feat/observer-foundation`; PR #13 зависит от PR #12 и не направлен непосредственно в main.
- CI run #330 на точном HEAD `05a3248eac1e3ccb8cdbd81bd72950a5f1eeffbe`: SUCCESS; rustfmt и Clippy PASS, 43 unit + 1 integration + 14 experiment contract + 16 observer integration = 74 теста PASS. Повторный Code Review запрошен на точном HEAD `05a3248`; новый review receipt, подтверждающий просмотр именно этого SHA, пока не зафиксирован.
- Предыдущие P2 исправлены: симметричный диапазон шума ограничен представимым диапазоном; квадратичные проверки заменены BTreeSet; общая часть хеша вычисляется один раз на observation. Дополнительно устранена неоднозначность конкатенации state_id и step: идентичность шума включает domain tag и length-prefix строковых идентификаторов.
- Предыдущие Code Review findings получили исправления и ответы; inline threads разрешены. Повторный Code Review запрошен на точном HEAD `05a3248`; новый receipt ещё не подтверждён.
- `visible_keys` определён как фильтр синтетической проекции, а не механизм авторизации; детерминированный шум явно не обеспечивает конфиденциальность или приватность.
- В `experiment_contract.rs` 14 контрактных тестов; CI run #330 подтвердил их прохождение на точном HEAD. Дополнительно добавлены 2 unit-теста на domain separation и однозначное кодирование state_id/step.
- Отдельный Security Review не имеет подтверждённого результата.
- В коде остаются синтетическое скрытое состояние, проекция, квантование, детерминированный шум и явное вмешательство. Полноценный runner, независимый контрольный измеритель, treatment/control/sham, статистические метрики и независимый verification pipeline не реализованы.
- Это синтетическая модель, не онтология реального смыслового поля и не доказательство свойств реального мира.

### PR #12 — Observer Model Foundation

Ссылка: https://github.com/WSOBIRW/SEMANTICGRAVITIFIELD/pull/12

- Состояние: OPEN, не слит.
- Ветка: `feat/observer-foundation`.
- Текущий HEAD после добавления CI trigger для feature-веток: `886704f6971835809b00bb4065871c29fff5f661` (код observer-модели не менялся в этом commit). CI run #323 на точном HEAD: SUCCESS; rustfmt, Clippy и тесты прошли.
- CI run #251 на старом HEAD `530dc07` был SUCCESS (41 unit + 1 integration + 12 observer tests). На текущем HEAD `3eca2ad` CI run #294: rustfmt PASS, Clippy PASS, 41 unit tests + 1 integration test + 16 observer integration tests PASS (58 всего). Промежуточные run #289 и #292 выявили соответственно ошибку тестового предположения о reverse одноэлементного списка и rustfmt-форматирование; оба исправлены до run #294.
- Добавлены ObserverDescriptor, ObservationProcedure и ObservationRecord.
- Добавлена структурная валидация идентичности, версий, временного интервала и ссылок на исходные доказательства.
- Добавлены расширяемые коды типов и явные значения Unknown для классификаций процедуры.
- Добавлены интеграционные позитивные и негативные тесты.
- Code Review receipt, доступный в истории PR, относится к более раннему commit `530dc07`; подтверждённого нового review receipt на точном HEAD `3eca2ad` в текущей проверке не обнаружено. Отдельный Security Review не имеет подтверждённого результата.
- Finding по подмене определения разрешён после ответа и обновления кода; inline thread resolved. PR не сливался.
- Структурная валидация не подтверждает истинность наблюдения, независимость наблюдателей или причинное влияние измерения.


- Новый P2 finding review: validate_against сравнивал только ID/version и пропускал подмену configuration_ref или других полей процедуры при сохранении ID/version. Исправлено каноническими SHA-256 digest полных ObserverDescriptor и ObservationProcedure; digest содержит domain separator, длины строк/списков, все поля и явные коды enum. ObservationRecord хранит оба digest; validate_against проверяет их вместе с ID/version.
- Добавлены негативные тесты подмены конфигурации процедуры, capability наблюдателя и digest в записи; тест детерминизма/порядка использует два элемента, чтобы реально проверять чувствительность к порядку.
- Digest не является подписью и не подтверждает происхождение. Если вызывающая сторона может одновременно заменить определение и digest записи, доверие не возникает; необходим внешний trust anchor для аутентификации.
- Finding reply отправлен и inline thread разрешён. Повторный code review и отдельный security review запрошены на новом HEAD; завершённые результаты пока не подтверждены.
## Не реализовано

- Исполняемая модель Observer/ObservationProcedure в main: первый типизированный слой находится в открытом PR #12 и не считается принятой реализацией.
- Машиночитаемое разделение состояния исследуемой системы, доступности наблюдения и представления наблюдателя.
- Измерение и независимая проверка влияния наблюдения на исследуемую систему.
- Механизмы сопоставления независимых/зависимых наблюдателей и сохранения коллективных разногласий.
- Исполняемый контролируемый эксперимент наблюдателей: создан только протокол `CONTROLLED_OBSERVER_EXPERIMENT.md`; генератор, наблюдатели, treatment/control runner, метрики и отчёт ещё не реализованы.
- Экспериментальная проверка гипотез о самонаблюдении и границах наблюдаемости.

- Независимый anti-rollback trust anchor вне failure domain файлового backend.
- Политика retention/GC с безопасностью для активных readers.
- session_id, stream generation и формальная reconnect identity в исполняемом ядре.
- Расширяемая схема событий и opaque preservation в коде.
- Миграция SGFS/SGFJ/SGFM между версиями.
- Out-of-order buffering и обработка поздних событий.
- Полная реконструкция рыночного состояния и completeness model.
- Полная provenance-модель производных данных в исполняемом коде.
- Semantic Intelligence и исследовательский движок.
- Независимый Verification Layer.
- Полная машиночитаемая схема внешнего контракта и SDK.
- Replay CLI.
- Полная реализация Decision Contract в коде.
- Независимая экономическая проверка эффективности.

## Известные ограничения текущей реализации

- Snapshot EventValidator сохраняет состояние валидатора, но не исходную историю и не состояние всего SGF.
- schema_version 1 — текущая поддерживаемая версия; стратегия совместимости нескольких поколений схем ещё не реализована.
- EventPayload пока содержит Quote и Trade; это MVP, не закрытый доменный перечень.
- event_id имеет глобальную область дедупликации в экземпляре валидатора; session/namespace пока отсутствуют.
- Sequence учитывается по source_id/instrument_id; отдельные каналы и поколения потока пока не моделируются.
- EventValidator::new() использует неограниченную дедупликацию; для production требуется явный бюджет памяти и ограниченное окно.
- Sequence gap обнаруживает пропуск, но не восстанавливает потерянные события.
- Reset sequence выполняется явным вызовом интегратора.
- Корректный replay доказывает воспроизводимость состояния валидатора в пределах журнала, но не полноту, достоверность или актуальность рынка.
- SHA-256 и hash chain сами по себе не обеспечивают аутентификацию и защиту от rollback.
- Unix file backend является первой реализацией, не ограничением архитектуры; NFS/SMB не заявлены поддерживаемыми без испытаний.
- CI PASS относится только к точному проверенному commit SHA.

## Ближайшие приоритеты

1. Получить и оценить отдельные security-focused reviews PR #12, #13 и #14 на точных текущих SHA; обработать findings и повторить проверки после каждого изменения.
2. Для PR #14 отдельно проверить модель доверия к storage root, lock-file path, hard links, generation/current path races, конкурентные writers и crash consistency. Не расширять обещания безопасности за пределы фактической модели доступа.
3. После закрытия review gates принять PR #12 в main, затем перенаправить/интегрировать PR #13 на принятый observer foundation и повторить CI/review на итоговом SHA.
4. После интеграции PR #12/#13 реализовать полноценный экспериментальный runner: генератор с seed, версионированную модель, скрытое состояние, treatment/control/sham, независимый контрольный измеритель, повторные прогоны, baseline, метрики и статистическую неопределённость.
5. Сохранить provenance, машинно-читаемый манифест и независимую реализацию ключевой метрики; не трактовать синтетические результаты как доказательство реального SGF.
6. Продолжить независимую проверку storage backend, сравнение file-generation и SQLite, проектирование anti-rollback trust anchor и безопасной retention/GC модели.
7. Развивать event/session identity, эволюцию схем, реконструкцию, Semantic Intelligence и Independent Verification отдельными доказательными этапами.

## Правило статуса

Статус «реализовано» означает, что код находится в main. Открытый PR, проект документа, интерфейс, тестовый сценарий или заявленная возможность не считаются частью принятой реализации. Любой PASS привязывается к точному commit SHA, конкретному CI run и набору проверок.
