# Статус реализации SGF

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
- Текущий HEAD: `82ed2644a76b6cb0393cb8d051b8ab0f444589da`.
- Реализация файлового backend перенесена в чистую ветку. Включены Cargo.toml, Cargo.lock, src/lib.rs, src/file_storage.rs и обновление CI workflow.
- Cargo.lock сгенерирован Cargo в успешном run #297; 21 пакет, включая платформенные зависимости Windows, с версиями и checksum. В CI Clippy и тесты теперь используют --locked. Временный экспорт lockfile как artifact удалён.
- Run #297 на кодовом HEAD `8e734ff`: rustfmt PASS, Clippy PASS, тесты PASS; Cargo.lock artifact получен и добавлен в репозиторий.
- CI run #314 на предыдущем HEAD `cf290ca`: rustfmt PASS, Clippy PASS, 58 unit tests + 1 integration test PASS (59 всего). Cargo.lock и `--locked` PASS. Source/license gate проверил 21 транзитивный пакет: все источники crates.io; лицензии соответствуют allowlist MIT/Apache-2.0/BSD-3-Clause, включая точную нормализацию legacy `MIT/Apache-2.0`.
- По P2 finding code review о пропущенной проверке RustSec advisory database добавлен отдельный gate: установка `cargo-audit` версии `0.22.2` с `--locked` и `cargo audit --file Cargo.lock`. CI run #317 на HEAD `0ae25f5` был запущен до последнего изменения workflow. После добавления `feat/**` в push/pull_request filters новый HEAD `82ed264` получил CI run #321: форматирование, Clippy и тесты прошли; установка cargo-audit выполнялась, advisory audit и source/license gate ещё не завершились.
- P2 finding по отсутствию advisory scan исправлен в коде workflow, но не считается закрытым до успешного CI и повторного review на новом HEAD.
- Реализованы конфигурируемые per-artifact и aggregate бюджеты чтения. Размеры всех файлов и исходная сумма проверяются до выделения буферов; aggregate budget повторно контролируется по фактически прочитанным байтам между последовательными чтениями, чтобы рост нескольких файлов после metadata check не обходил совокупный предел.
- Добавлены тесты на per-file budget, aggregate budget, рост файла, bounded reader при росте после metadata check и нулевой бюджет.
- Предыдущий Code Review на HEAD `cf290ca` выявил P2: отсутствовал аудит RustSec. Исправление добавлено в `0ae25f5`; повторный Code Review запрошен на новом HEAD `0ae25f5`; после последующего изменения CI workflow HEAD переместился на `82ed264`, поэтому итоговый review должен быть привязан к этому SHA. Завершённого review на `0ae25f5` пока нет.
- Отдельный Security Review не подтверждён. Проверка cargo-audit выявляет известные опубликованные advisories, но не устанавливает личность издателя, доброкачественность кода или отсутствие неизвестных уязвимостей.
- Локальный Rust toolchain в рабочей среде отсутствует; проверки выполняются GitHub Actions.
- Dependency gates разделены: Cargo.lock закрепляет версии и checksums; CI проверяет разрешённые registry sources и license expressions; cargo-audit проверяет известные RustSec advisories. Ни один из этих механизмов не подтверждает личность издателя, доброкачественность кода или отсутствие неизвестных уязвимостей.
- PR #14 не слит.

### PR #13 — Deterministic Synthetic Observer Experiment Core

Ссылка: https://github.com/WSOBIRW/SEMANTICGRAVITIFIELD/pull/13

- Состояние: OPEN, не слит.
- Ветка: `feat/controlled-observer-experiment`.
- Текущий HEAD: `22fe9597df40f714701d59dfe5d21f268f23acea`.
- Базовая ветка PR: `feat/observer-foundation`; PR #13 зависит от PR #12 и не направлен непосредственно в main.
- CI на точном HEAD `22fe959` не обнаружен; PASS не заявляется. Повторный запрос Code Review отправлен на этот SHA, но новый review receipt, подтверждающий просмотр именно этого SHA, пока не зафиксирован.
- Предыдущие P2 исправлены: симметричный диапазон шума ограничен представимым диапазоном; квадратичные проверки заменены BTreeSet; общая часть хеша вычисляется один раз на observation.
- Предыдущие Code Review findings получили исправления и ответы; inline threads разрешены. Наличие разрешённых старых threads не заменяет повторный review точного HEAD `22fe959`.
- `visible_keys` определён как фильтр синтетической проекции, а не механизм авторизации; детерминированный шум явно не обеспечивает конфиденциальность или приватность.
- В `experiment_contract.rs` 14 тестов, но они не запускались на точном HEAD после последнего изменения.
- Отдельный Security Review не имеет подтверждённого результата.
- В коде остаются синтетическое скрытое состояние, проекция, квантование, детерминированный шум и явное вмешательство. Полноценный runner, независимый контрольный измеритель, treatment/control/sham, статистические метрики и независимый verification pipeline не реализованы.
- Это синтетическая модель, не онтология реального смыслового поля и не доказательство свойств реального мира.

### PR #12 — Observer Model Foundation

Ссылка: https://github.com/WSOBIRW/SEMANTICGRAVITIFIELD/pull/12

- Состояние: OPEN, не слит.
- Ветка: `feat/observer-foundation`.
- Текущий HEAD после добавления CI trigger для feature-веток: `886704f6971835809b00bb4065871c29fff5f661` (код observer-модели не менялся в этом commit).
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

1. Завершить CI run #317 и повторный Code Review PR #14 на точном HEAD `0ae25f5`; затем получить отдельный Security Review и закрыть оставшиеся findings.
2. Получить повторный Code Review PR #12 на точном HEAD `3eca2ad`, отдельный Security Review и зафиксировать оба результата.
3. Получить CI и повторный Code Review PR #13 на точном HEAD `22fe959`, а также отдельный Security Review; сохранить зависимость от принятого PR #12.
4. После прохождения gates интегрировать PR #12, затем PR #13, повторяя проверки на итоговых SHA.
5. Продолжить независимую проверку модели угроз файлового backend: symlink/TOCTOU, конкурентные open/writers, crash consistency и локальный противник.
6. Сравнить file-generation backend и SQLite по требованиям, отказам и эксплуатации.
7. Определить независимый anti-rollback trust anchor и безопасную retention/GC модель.
8. Реализовать внутреннюю event/stream/session identity по IDENTITY_AND_TIME_MODEL.md и EVENT_SCHEMA_EVOLUTION.md.
9. Добавить v1 fixtures, legacy adapter, opaque preservation и отдельные версии форматов с миграционными тестами.
10. Продолжить Reconstruction, Semantic Intelligence и Independent Verification по отдельным доказательным этапам.

## Правило статуса

Статус «реализовано» означает, что код находится в main. Открытый PR, проект документа, интерфейс, тестовый сценарий или заявленная возможность не считаются частью принятой реализации. Любой PASS привязывается к точному commit SHA, конкретному CI run и набору проверок.
