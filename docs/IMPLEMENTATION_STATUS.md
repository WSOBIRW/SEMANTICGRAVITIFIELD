# Статус реализации SGF

## Текущая версия

Прототип: 0.1.0  
Язык ядра: Rust  
Репозиторий: WSOBIRW/SEMANTICGRAVITIFIELD  
Актуальная проверенная база документации на момент записи: main после коммита с обновлением ROADMAP.md.  
Назначение реализованного ядра: проверка входных рыночных событий, предотвращение повторов и некорректного порядка, snapshot/journal/replay и контракты долговечного восстановления.

## Архитектурная документация

Зафиксированы в main:
- ARCHITECTURE.md — назначение, контуры, инварианты и границы доверия.
- ARCHITECTURAL_CONSTRAINTS_REGISTER.md — постоянные инварианты, временные ограничения MVP, ограничения backend и правила эволюции.
- EVIDENCE_MODEL.md — доказательства, качество и независимость источников.
- DATA_PROVENANCE.md — происхождение данных и преобразования.
- UNCERTAINTY_MODEL.md — неполнота, неопределённость и воздержание.
- VERIFICATION_CONTRACT.md — независимая проверка и отрицательные контроли.
- SEMANTIC_INTELLIGENCE.md — исследование структур и гипотез.
- DECISION_CONTRACT.md — результаты и защитные вердикты.
- INTERFACE_CONTRACT.md — внешний контракт.
- IDENTITY_AND_TIME_MODEL.md — целевая модель идентичности и времени; реализация ещё не включена в ядро.
- ROADMAP.md — этапы развития.
- AUDIT_FINDINGS.md — findings раннего аудита.
- SNAPSHOT_FORMAT.md, JOURNAL_FORMAT.md, RECOVERY_MANIFEST.md — форматы восстановления.
- FILE_STORAGE_BACKEND.md — проект первого файлового backend.

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

**PR #11 — File Generation Recovery Storage**  
Ссылка: https://github.com/WSOBIRW/SEMANTICGRAVITIFIELD/pull/11  
Последний известный HEAD: c1523926a68149c16849d9ec441e67375ac5e1a7.  
Статус: открыт, не слит.

Реализовано в ветке PR:
- Unix file-generation backend на std::fs;
- CURRENT, .writer.lock, generations/ и .staging/;
- immutable generation directories со snapshot, journal и manifest;
- межпроцессная advisory-блокировка и CAS под lock;
- синхронизация файлов и каталогов и атомарная публикация CURRENT;
- проверки повреждений и crash injection tests.

GitHub Actions run #187 на указанном HEAD: fmt PASS, Clippy PASS, 50 unit tests PASS, 1 integration test PASS. Эти результаты относятся к конкретному SHA и не заменяют security review.

**PR #11 заблокирован актуальными незакрытыми review findings:**
- P1: долговечность записей вновь созданных root/ancestor directory entries в родительских каталогах;
- P1: нельзя удалять существующее поколение successor, пока не доказано, что оно безопасный orphan; восстановленный старый CURRENT не должен уничтожать более новое целое поколение;
- P2: защита от TOCTOU при проверке и перечислении .staging;
- P2: ограниченное чтение CURRENT до выделения памяти, не более фиксированного размера плюс один байт.

Три прежних review threads отмечены resolved, но четыре более поздних замечания остаются unresolved. PR не готов к слиянию. Устранение замечания автором и отметка thread как resolved сами по себе не являются независимым подтверждением исправления.

## Не реализовано

- Независимый anti-rollback trust anchor вне failure domain файлового backend.
- Политика retention/GC с безопасностью для активных readers.
- session_id, stream generation и формальная reconnect identity.
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

1. Закрыть четыре актуальных security review findings PR #11 и повторно проверить весь diff на точном новом HEAD.
2. Получить независимое ревью файловых операций и подтвердить CI на том же SHA; до этого не сливать PR.
3. Уточнить event/stream/session identity и temporal semantics через контракт и миграционный план.
4. Спроектировать расширяемую схему событий и безопасное сохранение неизвестных типов.
5. Добавить совместимость форматов и негативные тесты, не ломая SGFS v1 / SGFJ v1 / SGFM v1.
6. Продолжить Reconstruction, Semantic Intelligence и Independent Verification по отдельным доказательным этапам.

## Правило статуса

Статус «реализовано» означает, что код находится в main. Открытый PR, проект документа, интерфейс, тестовый сценарий или заявленная возможность не считаются частью принятой реализации. Любой PASS привязывается к конкретному SHA и конкретному набору проверок.
