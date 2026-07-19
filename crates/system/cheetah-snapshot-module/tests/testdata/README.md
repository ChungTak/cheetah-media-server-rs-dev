# Test fixtures

## golden_8x6.jpg

A minimal valid JPEG used as an MJPEG payload in integration tests.

- Dimensions: 8x6
- Color: red (RGB 255,0,0)
- SHA-256: `9208189deaa2dd9c36f36506932f3512bd1c1d30df2feb0a76c574c2ed1d8614`
- License: generated in-house; no third-party assets

Generation command:

```python
from PIL import Image
import io
img = Image.new('RGB', (8, 6), color='red')
buf = io.BytesIO()
img.save(buf, format='JPEG', quality=85)
open('golden_8x6.jpg', 'wb').write(buf.getvalue())
```
