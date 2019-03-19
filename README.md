# ctce8_cfg_tool

A tool for packing/unpacking ZTE Optical Modem configuration file

打包/解包中兴电信光猫 CTCE8 格式 cfg 配置文件工具


### 用法:

```PowerShell
.\ctce8_cfg_tool.exe unpack "E:\e8_Config_Backup\ctce8_ZXHN_F450.cfg" ctce8_ZXHN_F450.xml
.\ctce8_cfg_tool.exe pack ctce8_ZXHN_F450.xml ctce8_ZXHN_F450.cfg "ZXHN F450"
```

pack 打包命令的第三个参数是光猫设备名，一般为配置文件名中的字段。如我得到的配置文件名为 ctce8_ZXHN_F450.cfg，那么这个字符串就是 "ZXHN F450"，注意中间是空格。

准确描述见源码 [main.rs#L354](https://github.com/AlfnXd/ctce8_cfg_tool/blob/master/src/main.rs#L354)，Seek 跳过的部分就是这个字符串，可用 16 进制编辑器查看。

### 注意：

- 操作有风险，请做好备份工作之后再继续。
- 只在电信光猫 ZXHN F450 v2.0 上测试过。使用工具解包再打包回 cfg 文件，与光猫生成的 cfg 文件相同，修改后的重新打包的 cfg 文件也可以被光猫正确读取。
- 建议使用此工具解包而不是选择 [offzip](http://aluigi.altervista.org/mytoolz/offzip.zip) 等工具，因为解包代码中有仔细的校验逻辑，不兼容的 cfg 文件会有提示。

### 感谢

- [https://github.com/wx1183618058/ZET-Optical-Network-Terminal-Decoder](https://github.com/wx1183618058/ZET-Optical-Network-Terminal-Decoder)
