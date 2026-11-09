# Аудит реализации SGF PR #2–#8

## 1. Назначение
Документ фиксирует проверяемые расхождения между фактическим кодом и архитектурными требованиями SGF. Findings не означают автоматически, что соответствующая функция полностью отсутствует: указывается конкретный публичный путь, условие воспроизведения и требуемое исправление.

## 2. Область аудита
Проверены merged PR #2–#8, текущие реализации event, validation, prevention, snapshot, journal и recovery, а также замечания автоматического ревью в обсуждениях PR. Проверка выполнена по доступному состоянию main после архитектурного документирования.

## 3. Чем не является
Этот документ не является заявлением о полном security audit, формальной верификации или прохождении CI. Для каждого исправления требуются отдельные тесты и проверка CI на точном commit SHA. Отсутствие finding в этом документе не доказывает отсутствие дефектов.

## 4. Findings

### SGF-AUDIT-001 — Конфигурация ограниченной дедупликации недоступна через ErrorPreventionEngine
- Приоритет: P2.
- Статус: CLOSED в PR #9.
- Доказательство: merge commit `8b9503524e6dbf152d9dabc5c2233f101eac90ef`; PR CI на merge ref `fd19a8b8ced31340f2110334f86eba9a48065d7e` — fmt, Clippy и тесты PASS.
- Затронутый компонент: src/prevention.rs.
- Первоначальное условие: внешний потребитель использует публичный ErrorPreventionEngine::new()/inspect API.
- Первоначальное наблюдение: публичный EPE всегда создавал EventValidator с неограниченной дедупликацией.
- Первоначальный риск: длительно работающий потребитель мог неограниченно накапливать event_id.
- Исправление: добавлен ErrorPreventionEngine::with_dedup_capacity(NonZeroUsize), сохранив прежний new().
- Проверка: регрессионный тест подтверждает вытеснение старого ID из FIFO-окна через публичный API.

### SGF-AUDIT-002 — Reset потока недоступен через ErrorPreventionEngine
- Приоритет: P2.
- Статус: CLOSED в PR #9.
- Доказательство: merge commit `8b9503524e6dbf152d9dabc5c2233f101eac90ef`; PR CI на merge ref `fd19a8b8ced31340f2110334f86eba9a48065d7e` — fmt, Clippy и тесты PASS.
- Затронутый компонент: src/prevention.rs.
- Первоначальное условие: потребитель использует публичный inspect API после reconnect с перезапуском sequence.
- Первоначальное наблюдение: внутренний EventValidator имел reset_stream, но EPE не предоставлял forwarding method.
- Первоначальный риск: после reconnect sequence продолжала отклоняться как NonIncreasingSequence.
- Исправление: добавлен явный EPE::reset_stream(source_id, instrument_id); автоматический reset не добавлялся.
- Проверка: регрессионный тест подтверждает reset sequence и сохранение глобальной дедупликации event_id.

### SGF-AUDIT-003 — Неограниченное предварительное выделение памяти при восстановлении snapshot
- Приоритет: P2, безопасность доступности.
- Статус: CLOSED в PR #9.
- Доказательство: merge commit `8b9503524e6dbf152d9dabc5c2233f101eac90ef`; PR CI на merge ref `fd19a8b8ced31340f2110334f86eba9a48065d7e` — fmt, Clippy и тесты PASS.
- Затронутый компонент: src/snapshot.rs.
- Первоначальное условие: snapshot содержит checksum-valid, но сформированный извне счётчик ID.
- Первоначальное наблюдение: id_count использовался для HashSet::with_capacity до проверки bounded capacity.
- Первоначальный риск: чрезмерное предварительное выделение памяти по недоверенному счётчику.
- Исправление: bounded capacity проверяется до чтения ID; HashSet больше не резервирует память по недоверенному id_count.
- Проверка: регрессионный тест меняет счётчик, пересчитывает SHA-256 и подтверждает InvalidCapacity.

### SGF-AUDIT-004 — Монотонность recovery generation и защита от устаревшей записи
- Приоритет: P1 перед production storage.
- Статус: PARTIALLY ADDRESSED. Контрактный дефект закрыт в PR #10; атомарная реализация в backend и anti-rollback остаются OPEN.
- Доказательство контрактной части: merge commit `dc64586895b529b9aa617646f2af326a32c5600f`; CI на точном main SHA — fmt, Clippy и тесты PASS; ревью PR #10 завершено без findings.
- Затронутые компоненты: src/recovery.rs, RecoveryManifest, RecoveryStorage и будущий физический backend.
- Первоначальное наблюдение: manifest содержал generation, но commit_generation не задавал expected generation; автономная проверка пакета не могла установить, что поколение новее ранее принятого.
- Исправление PR #10: commit_generation принимает expected_current_generation; введены GenerationConflict, GenerationOverflow и InvalidNextGeneration; validate_generation_transition проверяет compare-and-swap и строгого successor; RecoveryPackage::validate проверяет пакет без его потребления.
- Реализованная гарантия: контракт теперь требует проверять фактическое поколение и публиковать successor внутри одной атомарной секции backend. Само наличие helper-функции не обеспечивает межпроцессную синхронизацию.
- Остаётся OPEN: реализовать CAS под OS lock/транзакцией; проверить два writer с одним expected generation; доказать crash consistency; добавить независимую доверенную monotonic checkpoint для защиты от отката всего хранилища.
- Критерий полного закрытия: конкретный backend и тесты покрывают stale writer, конкурентные commit, все точки аварии, повреждение и rollback относительно отдельного trust anchor.

## 5. Исправленные в последующих этапах замечания
- PR #2: отсутствие диагностики sequence gap устранено в последующей временной семантике; текущий EventValidator выдаёт SequenceGap warning и продвигает watermark.
- PR #7: reset_stream включён в журнал отдельной управляющей записью; replay применяет reset до следующих событий.
- PR #8: RecoveryManifest связывает snapshot, длину journal, anchor и expected_end; RecoveryPackage проверяет пакет и не возвращает частичный валидатор.

## 6. Критерии соответствия аудита
Finding закрывается только после изменения кода, добавления регрессионного теста, независимой проверки diff и подтверждённого CI для точного SHA. Изменение статуса в документации без доказательства исправления запрещено.
