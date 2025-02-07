from pathlib import Path

import pytest


@pytest.fixture()
def asset_dir() -> Path:
    return Path(__file__).parent / "assets"
