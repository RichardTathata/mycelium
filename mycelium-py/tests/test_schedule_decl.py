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
