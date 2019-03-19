# ctce8_cfg_tool

A tool for packing/unpacking ZTE Optical Modem configuration file

打包/解包中兴电信光猫 CTCE8 格式 cfg 配置文件工具

用法:

```PowerShell
.\ctce8_cfg_tool.exe unpack "E:\e8_Config_Backup\ctce8_ZXHN_F450.cfg" ctce8_ZXHN_F450.xml
.\ctce8_cfg_tool.exe pack ctce8_ZXHN_F450.xml ctce8_ZXHN_F450.cfg "ZXHN F450"
```
注意：

只在电信光猫 ZXHN F450 v2.0 上测试过，解包出的 xml 文件再打包回 cfg 文件，与光猫生成的 cfg 文件完全一致，可放心使用。

操作有风险，请做好备份工作之后再继续。
