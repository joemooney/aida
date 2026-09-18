# trace:STORY-1.A1 | ai:codex
def test_python_above():
    pass

class TestGroup:
    def test_python_inside(self):
        # trace:STORY-1.A2 | ai:codex
        assert True

def test_python_untraced():
    pass
