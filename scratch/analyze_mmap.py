with open("/tmp/osc_serial.log", "r") as f:
    for line in f:
        if "PageFault" in line:
            print(line.strip())
