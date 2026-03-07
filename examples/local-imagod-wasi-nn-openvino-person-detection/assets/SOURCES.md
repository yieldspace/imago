# Asset Sources

- `model.xml` / `model.bin`
  - Model: `person-detection-0202` (OpenVINO Open Model Zoo)
  - Source:
    - `https://storage.openvinotoolkit.org/repositories/open_model_zoo/2023.0/models_bin/1/person-detection-0202/FP16/person-detection-0202.xml`
    - `https://storage.openvinotoolkit.org/repositories/open_model_zoo/2023.0/models_bin/1/person-detection-0202/FP16/person-detection-0202.bin`
  - Reference docs: `https://docs.openvino.ai/2023.3/omz_models_model_person_detection_0202.html`

- `people.ppm`
  - Derived from:
    - `https://raw.githubusercontent.com/openvinotoolkit/model_server/releases/2023/3/demos/common/static/images/people/people1.jpeg`
  - Conversion:
    - `ffmpeg -i people1.jpeg -vf scale=512:512:flags=lanczos -pix_fmt rgb24 people.ppm`
