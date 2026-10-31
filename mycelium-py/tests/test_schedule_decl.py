"""Adopter-surface Y12, offline: the ``[[schedule]]`` a :class:`ScheduleDeclaration` sends."""

import pytest

from mycelium import ScheduleDeclaration


def test_the_declaration_is_the_unit_files_schedule_section() -> None:
    toml = ScheduleDeclaration("close", 60, ("ledger", "closer"), phase_secs=5, on_missed="run_once").toml()
    assert toml == (
        '[[schedule]]\nid = "close"\nperiod_secs = 60\nphase_secs = 5\non_missed = "run_once"\n'
        '[schedule.holders]\nns = "ledger"\nname = "closer"\n'
    )


def test_a_string_is_quoted_so_it_cannot_add_a_key() -> None:
    toml = ScheduleDeclaration('a"\nperiod_secs = 1', 60, ("ledger", "closer")).toml()
    assert toml.splitlines()[1] == 'id = "a\\"\\nperiod_secs = 1"'


@pytest.mark.parametrize("bad", [1.5, -1, True])
def test_a_period_that_is_not_whole_seconds_is_refused(bad: object) -> None:
    with pytest.raises(ValueError):
        ScheduleDeclaration("close", bad, ("ledger", "closer")).toml()  # type: ignore[arg-type]


def test_the_escaper_writes_what_toml_reads_back() -> None:
    import tomllib

    from mycelium import toml_basic_string

    for value in ['a"b\\c', "\b\t\n\f\r", "\x00\x01\x1f", "del\x7f", "🌱 é", "plain"]:
        assert tomllib.loads(f"v = {toml_basic_string(value)}\n")["v"] == value, repr(value)
    # A non-BMP character is written as itself, never as a surrogate pair (json.dumps's `🌱`).
    assert toml_basic_string("🌱") == '"🌱"'
    assert toml_basic_string("\x7f") == '"\\u007F"'


def test_the_declaration_writes_its_values_as_toml_reads_them() -> None:
    import tomllib

    decl = ScheduleDeclaration("close", 60, ("ledger", "closer 🌱\x7f"))
    holders = tomllib.loads(decl.toml())["schedule"][0]["holders"]
    assert holders == {"ns": "ledger", "name": "closer 🌱\x7f"}
