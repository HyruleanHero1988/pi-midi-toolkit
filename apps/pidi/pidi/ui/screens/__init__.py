"""Screen mixins composed into MidiToneApp."""
from pidi.ui.screens.fx import FxScreenMixin
from pidi.ui.screens.home import HomeScreenMixin
from pidi.ui.screens.kaoss import KaossScreenMixin
from pidi.ui.screens.kit import KitScreenMixin
from pidi.ui.screens.log import LogScreenMixin
from pidi.ui.screens.pads import PadsScreenMixin
from pidi.ui.screens.presets import PresetsScreenMixin
from pidi.ui.screens.seq import SeqScreenMixin
from pidi.ui.screens.settings import SettingsScreenMixin
from pidi.ui.screens.songs import SongsScreenMixin
from pidi.ui.screens.synth import SynthScreenMixin

__all__ = [
    "FxScreenMixin",
    "HomeScreenMixin",
    "KaossScreenMixin",
    "KitScreenMixin",
    "LogScreenMixin",
    "PadsScreenMixin",
    "PresetsScreenMixin",
    "SeqScreenMixin",
    "SettingsScreenMixin",
    "SongsScreenMixin",
    "SynthScreenMixin",
]
